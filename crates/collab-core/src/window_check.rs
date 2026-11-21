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
//! # When the check decides: strictly sequential imports only
//!
//! The check, and the rebuild of the id caches it relies on (`rebuild_id_cache_for`), are used for
//! **strictly sequential** imports and for nothing else. Every other import takes the full path
//! (`FallbackReason::NotSequential`): the id caches are rebuilt in full, exactly as `next` does
//! (`rebuild_id_cache`), and `semantic_snapshot` and `check_snapshot` decide. This is one rule, and
//! it replaces any reasoning about which concurrent shapes are harmless.
//!
//! Let `B` be the base's version (the op log's version vector before the import), `F` its frontier
//! and `L` the highest lamport of an op in `B`. An import is **strictly sequential** when
//!
//! 1. the base the facts describe is the one the import started from (`B` equals the facts'
//!    version vector);
//! 2. nothing is pending and every change the import added has been read (a causal import refuses
//!    otherwise);
//! 3. every change the import added has a lamport greater than `L`; and
//! 4. every change the import added causally depends on the whole of `B`: its deps contain every id
//!    of `F`, or one of its deps is an op of another change of the import that does.
//!
//! The facts record `F` and `L` in the unmetered setup (`facts::BaseVersion`); `L` is the lamport of
//! a head, because a lamport grows along every dependency edge. The lamport of a change is never
//! what the client wrote: the library recomputes it from the change's deps when it imports one
//! (`import_changes_to_oplog`, `try_get_change_lamport_from_deps`, in `loro-internal` 1.18), so
//! clause 3 follows from clause 4 and is checked as a second, independent test. An update with no
//! change in it (already known) is vacuously sequential.
//!
//! ## Why a strictly sequential import adds nothing no op of it names
//!
//! The library applies an import in one of four modes (`DiffMode`, `loro-internal`
//! `diff_calc.rs`). `Linear` and `ImportGreaterUpdates` apply the imported ops one by one, in
//! lamport order: only the imported ops act. The engine never subscribes, so the state does not
//! record events and the ops go through `TreeState::apply_diff` (`tree_state.rs`), not through the
//! event-recording path. `ImportGreaterUpdates` checks the parent of a move and drops an op whose
//! parent check fails. `Linear` does not check (`need_check` is false) and leaves a node whose
//! parent the document never had in the state with a dangling parent, which the projection refuses
//! when the node is named and ignores otherwise. Nothing panics in the worker in either mode, but a
//! subscribed client does panic on such an op, so `crate::tree_refs` refuses every tree op that
//! names a parent that does not exist when it runs, before this check, with or without facts
//! (`r3_8_a_create_under_a_missing_parent_is_refused`); `next` accepted it. The argument below does not
//! depend on the mode: it needs only that an imported op acts on the node it names, and that
//! nothing else is added or moved, which `created_nodes_adopt_nothing` checks on the document after
//! the import. A worker panic is a refusal, so a panic in some shape would be `next`'s behaviour as
//! well.
//!
//! `Import` and `Checkout` recompute the tree from the history cache
//! (`diff_calc/tree.rs` `checkout_diff`): they retreat every cached op whose lamport is at or above
//! the lowest lamport of an op in the version difference, and replay it in (lamport, peer) order.
//! That is how a concurrent update can add a node no op of it names: the replay of a create
//! emits a `Create` for every child the cache holds under it (`push_children_creation`), among them
//! a node whose own create ran, and was dropped, before its parent existed, and it replays and
//! moves base ops that sit above the lowest new lamport.
//!
//! For a strictly sequential import the version difference is the imported ops alone and, by clause
//! 3, its lowest lamport is above `L`. The retreat range is `[lowest, cache maximum]` and the cache
//! holds only ops of `B`, so it is **empty**: nothing of the base's history is retreated or replayed,
//! in any mode (`ImportGreaterUpdates` and `Linear` never look at it at all). The only ops that act
//! on the tree are the imported ones. Each acts on the tree through its own target: a create
//! creates its target; a move or delete changes its target (those are not decided here, see
//! `TreeStructure`); a map or text op writes the container it names. The one effect an imported op
//! has on a node it does not name is the adoption by `push_children_creation`: a create of `X`
//! emits `Create` for the nodes the history holds under `X`, which exist only if the base holds
//! an op that names `X` as a parent before `X` existed (a hand-encoded create or move; the library
//! does not check that a parent id was ever created). That is the one case that remains, and it is
//! checked on the document after the import: no created node has a child the update did not create
//! (`created_nodes_adopt_nothing`, `ResolvedNodeId`). Anything else a replay could add, move, revive
//! or re-parent needs a base op above the lowest new lamport, and clause 3 rules that out.
//!
//! The predicate allows concurrency *inside* the import: several changes that each depend on the
//! whole base and not on each other. The library applies them in lamport order, so a child whose
//! parent is created by a concurrent change of the same import with a higher lamport is dropped from
//! the state, while the history keeps its create: a later concurrent import replays it into the
//! state (round two's R2-4 planting, done by one sequential update). The worker's state and the
//! host's snapshot agree, so the decision is right, and `next` accepts the same update, but every
//! later concurrent write to that page takes the full path and is refused by `tree_depth` once the
//! hidden part is deep. This is older than the window and is documented here, not changed.
//!
//! Where the cache rebuild was incremental and the check then falls back, the id caches are
//! completed before the full computation reads them (`LoroCollabEngine::complete_id_cache`), so
//! the full computation reads what `next`'s rebuild gives.
//!
//! # Which units an update touched
//!
//! The update is hostile input, so "touched" is read from the import itself, never from what the
//! client claims: the ops the library added to the op log (`floor_guard::scan_imported_ops_visiting`,
//! one pass shared with the downgrade guard). Each op names its container, and a container id is the
//! id of the op that created it, so it cannot be faked. A container is mapped to its unit by its
//! path from the root in the document after the import (`LoroDoc::get_path_to_container`), which is
//! state, not a claim. **A container that has no path is not "unread"**: the library gives the
//! metadata map of a deleted tree node, or of a node whose map is registered under another tree,
//! no path, and the projection reads it all the same. A container without a path, and a container
//! whose path goes through a tree node but does not start at the engine's tree, is `Unclassified`
//! and takes the full path. What cannot be classified is never taken to be irrelevant.
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
//!    the base has no placement structure, no duplicate logical id, projects, is within every limit
//!    and within the work budget of [`crate::work`] (`BaseFacts::compute`); the facts' version
//!    vector equals the import's `before_state`.
//! 2. The import is strictly sequential (`NotSequential`), as defined above, and left nothing
//!    pending (`Unclassified`).
//! 3. No op touched the `doc` map, the structure meta or tree, a shadow root, or a tree other than
//!    the engine's (`GlobalStructure`, `TreeStructure`); no op on the engine tree is anything but
//!    a create whose target is its own op id (`TreeStructure`); no map op writes or deletes the key
//!    `logical_id` except in the metadata map of a node the update created (`IdRewritten`, and for
//!    the map of an existing tree node a refusal, see "The refusal of an id rewrite"); every
//!    touched container has a path, and none under a tree node is reached through any other root
//!    (`Unclassified`).
//! 4. Every element inserted into the root list is a map container created by the inserting op
//!    itself, and no element of another kind was inserted (`UnreadableInsert`).
//! 5. Every created target is a node the tree did not hold before the import (the two id caches
//!    together list every node of the base tree, live or deleted, named or not), no created node
//!    has a child that the update did not create, and no two nodes claim one id
//!    (`ResolvedNodeId`, `DuplicateLogicalId`). A write of an existing node's id never reaches
//!    this clause: it is refused before the check.
//! 6. The items (see [`crate::work`]) of the base plus an upper bound on what the update added are
//!    within the budget of the exact post-import totals (`WorkBudget`).
//! 7. Every touched unit is valid for the reader and no key crosses between the two kinds of unit;
//!    no existing tree node enters or leaves the projection; the alias map is within its limit; no
//!    deepest or longest unit is over its limit (`InvalidUnit`, `KeyCollision`, `PresenceChange`,
//!    `AliasLimit`, `LimitReached`); and the facts and the delta agree (`Inconsistent`).
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
//!   import runs through the block (condition 3). So the blocks whose evaluation can differ are
//!   the inserted ones, the removed ones and the ones some op's container maps to.
//! * **Legacy tree nodes.** A node's contribution depends on its kind, its text, its
//!   `content_source`, its key, its deleted state (its own and its ancestors') and its depth (its
//!   parent chain). A strictly sequential import replays no base op (above), and condition 3 leaves
//!   it no move and no delete, and no write of an existing node's key, so no existing node changes
//!   parent, ancestors, key or depth; an existing node can change only through an op on its own
//!   metadata map, which maps to it by path (condition 3: a container without a path falls back). A
//!   created node is new, exists only because of its own create op and has no child but created
//!   ones (condition 5), so it has no earlier state to differ from and nothing existing hangs under
//!   it; its depth is its parent chain through created nodes and then the base's depth of an
//!   existing parent, which has not changed. So the nodes whose evaluation can differ are the
//!   created ones and the ones some op's container maps to. A node without a logical id is not in
//!   the projection and counts toward nothing.
//! * **Totals.** Each total is the sum of per-unit contributions, so the total after the update is
//!   the base's plus the sum, over the touched units, of the contribution now minus the base's
//!   contribution: exact. Every other unit contributes what it did.
//! * **Maxima.** The base is within the depth and longest-text limits (a precondition of the
//!   facts), and every unit that is not touched has the depth and text it had, so a maximum can only
//!   be over its limit in a touched unit, and each touched unit is checked, the check stopping at
//!   the first one over. The depth and longest bounds in the report are the larger of the base's
//!   maximum and the touched units' values, so they are at least the exact maxima.
//!
//! # What takes the full path
//!
//! The full computation decides, with the id caches rebuilt in full as on `next`: every import that
//! is not strictly sequential (any concurrency with the base: a client on an older version, two
//! clients editing at once; any update that is not a single causal extension of the whole base);
//! pages with a placement structure (editor models 4 and 5); a base with duplicate logical ids, one
//! the projection or the limits do not accept, or one over the work budget; every legacy tree move
//! or delete; any op on the `doc` map, the structure containers or a tree other than the engine's;
//! a created node that already existed or has children the update did not create; a write of the
//! key `logical_id` into a map that is not a tree node's metadata (a rewrite of an existing node's
//! id is refused instead); an update that could take the page over the work budget; a touched
//! container without a path (the metadata of a deleted node, nodes created under a deleted
//! parent); and every shape condition 7 names. With the configuration switch off (no facts) every
//! window is this path, and the **worker** is `next`'s computation for every input but one (the tree
//! reference refusal of `crate::tree_refs`, which holds with the switch off too): the same import,
//! the same full id-cache rebuild, the same projection and limits (the differential test
//! `with_the_switch_off_the_window_is_next_on_every_case` keeps a copy of `next`'s import as its
//! oracle, and compares the worker only). The **host** is not `next`: after every accepted window,
//! with the switch on or off, it projects the document it received and refuses (an internal error)
//! one that is over a limit. `next` committed such a document in one known case, a rewrite of an
//! existing node's logical id (see "The refusal of an id rewrite"); the corpus holds that input
//! (`round3/id_rewrite`) and the test records the host's refusal.
//!
//! # The refusal of an id rewrite
//!
//! With facts (the switch on), an update that writes or deletes the `logical_id` of a tree node that
//! existed before it is **refused** (`OperationFailed`, reason [`ID_REWRITE_REASON`]) before the
//! check, whatever the import's shape. The id caches never re-read a node they hold (a limitation
//! older than this module, see `rebuild_id_cache`), so the worker keeps the old id of a renamed node
//! while the host's fresh load reads the new one, and the two projections can differ: on `next` the
//! write is committed even when the host's projection of the result is over a limit. No client
//! writes an id after the node's creation (the editor sets it in the transaction that creates the
//! node; the engine inserts it with the create), so nothing legitimate is refused. Without facts
//! (the switch off, or a base the check does not describe) the write is `next`'s: accepted by the
//! worker, and the host refuses a result that is over a limit.
//!
//! # What the incremental path covers
//!
//! After this narrowing the incremental path covers the updates of a client that wrote on top of
//! the page's current head: one client typing, inserting and deleting blocks, and several clients
//! when each one's update is made after it received the previous accepted update. An update made
//! on an older version of the page (the other client's update was accepted while this one was in
//! flight or being typed) falls back. Measured on the simulation in
//! `window_calibration_tests::window_realistic_fraction` (flat page of 300 blocks, 80% typing and
//! 20% block inserts; writes at random intervals around the period; the server's accepted update
//! reaches the other client one latency after the update reaches the server):
//!
//! | clients | write period per client | one-way latency | decided incrementally |
//! | --- | --- | --- | --- |
//! | 1 | 300 ms | 40 ms | 100% |
//! | 2 | 5,000 ms | 100 ms | 97% |
//! | 2 | 2,000 ms | 100 ms | 91% |
//! | 2 | 1,000 ms | 100 ms | 82% |
//! | 2 | 500 ms | 100 ms | 67% |
//! | 2 | 200 ms | 20 ms | 84% |
//! | 2 | 200 ms | 100 ms | 32% |
//! | 2 | 200 ms | 300 ms | 12% |
//!
//! A fallback that comes after the units were evaluated (the work budget, a presence change, a key
//! collision, a bound reached) runs the full computation from the start, so such an update costs
//! about the evaluation plus the full computation, and a CPU refusal of that is not retried.
//! Avoiding it needs either the budget and the presence conditions known before evaluating (the
//! budget needs the exact totals) or a retry with the full mode on the wire; neither is small, and
//! it only matters for a page whose full computation already takes most of the window, for an update
//! that touches most of its blocks and then falls back. Concurrent updates (`NotSequential`) return
//! before any evaluation and are not affected.
//!
//! The fallback runs the full computation, which on a page of more than about 3,000 flat blocks (or
//! the equivalent in legacy nodes) does not fit the 50 ms window on the reference runner: for such a
//! page a concurrent update is refused by the CPU limit, exactly as every update of it was before
//! the incremental window existed.
//!
//! # The work budget
//!
//! Content that no documented limit counts is not paid for by the incremental window, so the page's
//! size is bounded separately (`crate::work`): the base's items are counted in setup (no facts above
//! the budget), and the window keeps the count with an upper bound on what the update added and falls
//! back above the budget (`WorkBudget`). Measured on the development machine (release), the cost of
//! the full window per item is between 0.1 and 0.6 microseconds; the budget's uncounted part is
//! 16,000 items, which costs the full window at most about 6 ms there, 21 ms at 3.6 times that, well
//! under 50 ms. See `crate::work` for the unit, the budget and the table.
//!
//! The payload of text marks and the internal pieces of a text are the one part the ops cannot bound (a
//! mark that splits a text adds segments that carry the values of other marks, and a mark over a
//! fragmented text adds an attribute to every piece). The facts keep the mark items per editor block,
//! and `recount_marks` re-counts the touched blocks after the import and before any is evaluated.
//! The value walk stops at the headroom the budget has left; the conversion of a text's delta itself
//! cannot stop early, and is bounded by the base figure (which the facts counted) and by the metered
//! import (which paid for every attribute the update added).
//!
//! The incremental window is O(the update): its id-cache rebuild reads the created nodes only (a
//! node without a logical id gains one only through a write of the key, which falls back). The
//! work-budget allowances are fungible across kinds of content, so the budget bounds the uncounted
//! content at about `WORK_UNCOUNTED_ITEMS` plus whatever the live totals leave unspent, which is
//! what the full path (setup, host projection) pays for, not the incremental window.

use std::collections::{BTreeMap, HashMap, HashSet};

use loro::{
    Container, ContainerID, ContainerType, Index, JsonListOp, JsonMapOp, JsonOp, JsonOpContent, JsonTreeOp, TreeID,
};

use crate::engine::{ImportTrace, LoroCollabEngine, TreeUnit};
use crate::error::CollabError;
use crate::facts::{BaseFacts, Contribution, UnitFact};
use crate::isolation::wire::WindowDecision;
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
        /// Does not fall back when the page's items (base plus update) are over the budget.
        WorkBudgetSkipped,
        /// Does not re-count the mark payload of the touched texts.
        MarkRecountSkipped,
        /// Does not count the internal pieces of a text, only its merged segments.
        PieceCountSkipped,
        /// Treats every import as strictly sequential: rebuilds the id caches from the update's own
        /// ops and decides incrementally even when the update is concurrent with the base.
        SequentialGateRemoved,
        /// Skips a container that has no path as unreachable, as the first version of the check did.
        PathlessContainerSkipped,
        /// Does not fall back when an update writes the logical id of an existing tree node.
        IdRewriteSkipped,
        /// Reports a limit refusal with an `observed` value one too high.
        RefusalOffByOne,
        /// The setup's allocation guard never fires: the base facts are built whatever the setup
        /// has cost (`crate::setup_guard`).
        SetupGuardSkipped,
        /// The setup's CPU guard never fires (`crate::setup_guard`).
        SetupCpuGuardSkipped,
        /// The setup's per-text budget never fires (`crate::setup_guard`): a text that alone costs
        /// more than the window's CPU ceiling still yields facts.
        TextGuardSkipped,
        /// The window never tells its caller what the check decided: the worker writes no decision
        /// record, so the host cannot know the update was not sequential once a ceiling kills it.
        DecisionNotReported,
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
    /// A created target already existed in the tree (live, deleted or unnamed), a created node has
    /// children the update did not create, or the id caches were rebuilt in full. A write of an
    /// existing node's id is not this: it is refused before the check ([`ID_REWRITE_REASON`]).
    ResolvedNodeId,
    /// An update wrote the key `logical_id` in a map that is not the metadata of a node it created
    /// (the write of an existing tree node's id is refused before the check, see
    /// [`ID_REWRITE_REASON`]; this is any other map).
    IdRewritten,
    /// The import is not a strict causal extension of the base (see the module documentation):
    /// some change of it is concurrent with the base, or does not depend on the whole of it.
    NotSequential,
    /// The page's items, the base's plus an upper bound on what the update added, are over the
    /// budget of the page's live totals (`crate::work`).
    WorkBudget,
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
/// `on_decision` is called exactly once, as soon as the incremental check has decided and before
/// anything else runs: before the full computation of a fallback, and before returning on an
/// accepted or refused check. It is not called when the import itself is refused (no check ran).
/// The worker uses it to tell the host the decision before a ceiling can kill it.
///
/// # Errors
/// Everything `import_client_update` refuses, and the refusal of the full computation (a projection
/// error or a `LimitExceeded`), identical to what the full computation alone reports.
pub fn run_window(
    engine: &mut LoroCollabEngine,
    facts: Option<&BaseFacts>,
    update: &[u8],
    on_decision: &mut dyn FnMut(WindowDecision),
) -> Result<WindowAccepted, CollabError> {
    let trace = engine.import_client_update_traced(update, facts.map(|facts| &facts.version))?;
    let checked = check(engine, facts, &trace);
    if !injected!(DecisionNotReported) {
        on_decision(match &checked {
            Ok(_) => WindowDecision::Incremental,
            Err(Undecided::IdRewrite | Undecided::Refused(_)) => WindowDecision::Refused,
            Err(Undecided::Fallback(reason)) => WindowDecision::Fallback(*reason),
        });
    }
    match checked {
        Ok(report) => Ok(WindowAccepted {
            report: Some(report),
            fallback: None,
        }),
        // A write of the logical id of a node that existed before the update: refused, see
        // `rewrites_an_existing_nodes_id`.
        Err(Undecided::IdRewrite) => Err(CollabError::OperationFailed {
            reason: ID_REWRITE_REASON.into(),
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
            // The full computation reads the id caches: after an incremental rebuild they are made
            // what `next`'s full rebuild gives (a no-op after a full one).
            engine.complete_id_cache()?;
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
    /// The changes the import added, as stored: where they start and end, their lamport and deps.
    changes: Vec<ChangeSpan>,
    /// A change of the import whose length could not be read: nothing about the import is known.
    unreadable_change: bool,
    /// An upper bound on the items (see `crate::work`) the ops of the import added to the page.
    work_added: crate::work::OpWork,
    /// An op of the import put a style mark on a text (or ended one): the only way a text gains
    /// marks, so without it a text that had none has none.
    mark_ops: bool,
    /// The `create` and `move` ops of every tree the import names (`crate::tree_refs`).
    tree_refs: crate::tree_refs::TreeRefs,
}

/// One change of an import, reduced to what the sequential gate reads.
#[derive(Debug, Clone)]
struct ChangeSpan {
    peer: u64,
    /// The counters of its ops: `start..end`.
    start: i32,
    end: i32,
    lamport: u32,
    deps: Vec<loro::ID>,
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
    /// Refuses an import whose tree ops name a parent that does not exist when the op runs, or
    /// move a node below itself (`crate::tree_refs`).
    pub(crate) fn refuse_unresolved_tree_parents(
        &self,
        doc: &loro::LoroDoc,
        before: &loro::VersionVector,
    ) -> Result<(), CollabError> {
        self.tree_refs.verdict(doc, before)
    }

    /// Whether `tree_created` is every node the import added to the legacy tree.
    pub(crate) const fn tree_ids_known(&self) -> bool {
        !self.seen.has(Seen::TREE_OTHER)
    }

    /// Whether an op of the import wrote or deleted the `logical_id` of a legacy tree node that
    /// existed before the import: a write to the `logical_id` key of the metadata map of a node
    /// that the import did not create (a node's metadata map is named by the node's id).
    ///
    /// The id caches never re-read a node they hold, so the worker would keep the node's old id
    /// while the host's fresh load reads the new one: the two projections can differ, and
    /// `next` commits a document the host's projection finds over a limit. No client writes an id
    /// after the node's creation (the editor and the engine set it in the transaction that creates
    /// the node), so the window refuses such a write. Only used when the incremental window is on.
    pub(crate) fn rewrites_an_existing_nodes_id(&self, engine: &LoroCollabEngine) -> bool {
        let created: HashSet<TreeID> = self.tree_created.iter().copied().collect();
        self.logical_id_writes.iter().any(|container| match container {
            ContainerID::Normal {
                peer,
                counter,
                container_type: ContainerType::Map,
            } => {
                let node = TreeID::new(*peer, *counter);
                // "Existing" is read after the import: a node the import itself added without a plain
                // create of this update counts as existing, which only hand-encoded input produces.
                !created.contains(&node) && engine.has_tree_node(node)
            }
            _ => false,
        })
    }

    /// Reads one imported change, before its ops.
    pub(crate) fn visit_change(&mut self, change: &loro::JsonChange) {
        self.tree_refs.visit_change(change);
        let Some(len) = change.checked_op_len().and_then(|len| i32::try_from(len).ok()) else {
            self.unreadable_change = true;
            return;
        };
        let Some(end) = change.id.counter.checked_add(len) else {
            self.unreadable_change = true;
            return;
        };
        self.changes.push(ChangeSpan {
            peer: change.id.peer,
            start: change.id.counter,
            end,
            lamport: change.lamport,
            deps: change.deps.clone(),
        });
    }

    /// An upper bound on the items the import's ops added to the page (see `crate::work`).
    pub(crate) const fn work_added(&self) -> u64 {
        self.work_added.total_items()
    }

    /// The items (without the payload) the import's ops added, which also bound how many blocks and
    /// navigators it can have added.
    pub(crate) const fn work_added_items(&self) -> u64 {
        self.work_added.items
    }

    /// Whether any op of the import put a style mark on a text.
    pub(crate) const fn has_mark_ops(&self) -> bool {
        self.mark_ops
    }

    /// Whether the import is a **strict causal extension** of the base: the one condition under
    /// which the incremental id-cache rebuild and the incremental check are used (see the module
    /// documentation).
    ///
    /// Let `B` be the base's version (`before_ops`), `F` its frontier and `L` the highest lamport
    /// of an op in it. The import is strictly sequential when
    ///
    /// 1. the base the facts describe is the one the import started from (`base.oplog_vv ==
    ///    before_ops`);
    /// 2. every change of the import has a lamport greater than `L`; and
    /// 3. every change of the import causally depends on the whole of `B`: either its deps contain
    ///    every id of `F`, or one of its deps is an op of another change of the import that does.
    ///
    /// An import without any change (the update was already known) is vacuously sequential.
    pub(crate) fn strictly_sequential(
        &self,
        base: &crate::facts::BaseVersion,
        before_ops: &loro::VersionVector,
    ) -> bool {
        if self.unreadable_change || &base.oplog_vv != before_ops {
            return false;
        }
        let mut order: Vec<&ChangeSpan> = self.changes.iter().collect();
        order.sort_by_key(|change| change.lamport);
        // The counter ranges `start -> end` of the changes seen so far, by peer. Every change seen
        // depends on the whole base (the loop returns at the first one that does not), and the
        // ranges of one peer do not overlap: a lookup is the last range that starts at or before the
        // counter. A hostile overlap can only make the lookup miss, which is not sequential.
        let mut seen: HashMap<u64, BTreeMap<i32, i32>> = HashMap::new();
        for change in order {
            if base.max_lamport.is_some_and(|highest| change.lamport <= highest) {
                return false;
            }
            let deps: HashSet<loro::ID> = change.deps.iter().copied().collect();
            let covers_frontier = base.frontier.iter().all(|head| deps.contains(head));
            let through_import = !covers_frontier
                && change.deps.iter().any(|dep| {
                    dep.counter >= before_ops.get(&dep.peer).copied().unwrap_or(0)
                        && seen
                            .get(&dep.peer)
                            .and_then(|ranges| ranges.range(..=dep.counter).next_back())
                            .is_some_and(|(_, end)| dep.counter < *end)
                });
            if !covers_frontier && !through_import {
                return false;
            }
            let end = seen
                .entry(change.peer)
                .or_default()
                .entry(change.start)
                .or_insert(change.end);
            *end = (*end).max(change.end);
        }
        true
    }

    /// Reads one imported op. `peer` is the peer of the change that carries it and `root_list` the
    /// id of `doc.children` after the import.
    pub(crate) fn visit(&mut self, peer: u64, op: &JsonOp, root_list: Option<&ContainerID>) {
        self.tree_refs.visit(peer, op);
        self.work_added = self.work_added.plus(crate::work::op_work(&op.content));
        if matches!(
            op.content,
            JsonOpContent::Text(loro::JsonTextOp::Mark { .. } | loro::JsonTextOp::MarkEnd)
        ) {
            self.mark_ops = true;
        }
        match &op.container {
            // A mergeable child of a map (the text of a legacy tree node, for one) is named like a
            // root container but belongs to the map that created it: it is mapped to its unit by
            // its path, like any other child.
            ContainerID::Root { .. } if op.container.is_mergeable() => {
                self.containers.insert(op.container.clone());
            }
            ContainerID::Root { name, container_type } => self.visit_root(peer, op, name, *container_type),
            ContainerID::Normal { .. } if Some(&op.container) == root_list => self.visit_root_list(peer, op),
            // An op on a tree that is not a root container: no part of the window's model needs
            // it, and it can register a node's metadata map under it.
            ContainerID::Normal {
                container_type: ContainerType::Tree,
                ..
            } => self.seen.set(Seen::TREE_OTHER),
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
    /// The update writes the logical id of a tree node that existed before it: refused.
    IdRewrite,
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

/// The reason of the refusal of a write of an existing tree node's logical id.
pub const ID_REWRITE_REASON: &str = "logical_id_rewritten";

const CID_PREFIX: &str = "cid:";
/// The key of a tree node's metadata map that holds its logical id.
const LOGICAL_ID_KEY: &str = "logical_id";

/// `Ok(report)` when the update is certainly accepted by the full computation too, and otherwise
/// either the reason the check did not decide or the refusal of a total.
fn check(engine: &LoroCollabEngine, facts: Option<&BaseFacts>, trace: &ImportTrace) -> Result<WindowReport, Undecided> {
    let facts = facts.ok_or(FallbackReason::FactsAbsent)?;
    if !injected!(IdRewriteSkipped) && trace.summary.rewrites_an_existing_nodes_id(engine) {
        return Err(Undecided::IdRewrite);
    }
    // A causal import never leaves anything pending; if it did, the ops seen are not the update.
    if trace.status.pending.is_some() {
        return Err(FallbackReason::Unclassified.into());
    }
    if facts.state != trace.before_state {
        return Err(FallbackReason::StaleFacts.into());
    }
    // Only a strict causal extension of the base is decided here: any concurrency with the base
    // can make the library replay base history and add or move nodes no op of the update names.
    if !trace.sequential {
        return Err(FallbackReason::NotSequential.into());
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
    // The mark payload of the touched texts is re-counted before any unit is evaluated: evaluating a
    // text reads its marks once per segment, which is the work this count bounds.
    let work_added = summary.work_added();
    let mark_items = recount_marks(engine, facts, &touched, summary, work_added)?;
    evaluate_blocks(engine, facts, &touched, &limits, &mut delta)?;
    evaluate_tree_nodes(engine, facts, &created, &touched, &limits, &mut delta)?;
    finish(facts, work_added, mark_items, &delta, &limits)
}

/// The growth of the touched blocks' mark items (see `crate::work`), or the work-budget fallback when
/// the budget cannot hold it.
///
/// What bounds the re-count: the budget of the page after the update is at most that of the base's
/// totals plus one block and one navigator per op (an op names at most one unit, so it adds at most
/// one live block or navigator) and the documented character limit, with the formatting allowance
/// of every such block. The headroom is that budget
/// less the base's items and the update's own bound, and a block's value walk stops once its items
/// pass its base figure plus the headroom still left. The conversion of a text's delta cannot stop
/// early: it costs what the base figure says (which the facts counted and the evaluation of the
/// block pays again) plus what the metered import paid for the attributes the update added. Only
/// the inline texts that the block reader converts are read at all.
fn recount_marks(
    engine: &LoroCollabEngine,
    facts: &BaseFacts,
    touched: &Touched,
    summary: &OpSummary,
    work_added: u64,
) -> Result<u64, FallbackReason> {
    use crate::limits::DOCUMENT_TEXT_CHARS_MAX;
    if injected!(MarkRecountSkipped) {
        return Ok(0);
    }
    let growth = summary.work_added_items();
    let cap = crate::work::item_budget_with_marks(
        (facts.blocks as u64).saturating_add(growth),
        (facts.navigators as u64).saturating_add(growth),
        DOCUMENT_TEXT_CHARS_MAX as u64,
        u64::MAX,
    );
    let spent = facts.items.saturating_add(work_added);
    engine
        .mark_items_added(
            facts,
            &touched.blocks,
            summary.has_mark_ops(),
            cap.saturating_sub(spent),
        )
        .ok_or(FallbackReason::WorkBudget)
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
        // A container without a path is not "unread": the metadata map of a deleted tree node has
        // none (the library answers `None` for a child of a deleted node), and the projection
        // reads it all the same, and so does a map registered under another tree. What cannot be
        // classified is never taken to be irrelevant.
        let Some(path) = doc.get_path_to_container(id) else {
            if injected!(PathlessContainerSkipped) {
                continue;
            }
            return Err(FallbackReason::Unclassified);
        };
        let Some((ContainerID::Root { name, container_type }, _)) = path.first() else {
            return Err(FallbackReason::Unclassified);
        };
        if crate::reserved_roots::is_shadow_root(name, *container_type) {
            return Err(FallbackReason::GlobalStructure);
        }
        // A tree node on the path of a container under any root but the engine's tree means the
        // library reached the container through a tree this check does not model (a metadata map
        // registered under another tree, or a tree nested in a block): not mapped.
        let through_node = path.iter().any(|(_, index)| matches!(index, Index::Node(_)));
        match name.as_str() {
            DOC if !through_node => match block_of(&path) {
                Some(block) => {
                    touched.blocks.insert(block);
                }
                // The list itself, which the summary handles by id, would be the only other
                // legitimate path; anything else under `doc` is not modelled.
                _ => return Err(FallbackReason::GlobalStructure),
            },
            // Exactly one tree node on the path, and it is the engine node at position 1. A deeper
            // `Index::Node` means the container belongs to a node whose metadata map is registered
            // under a tree nested in another node's metadata: attributing it to the outer node
            // would miss the node the projection reads it through.
            ENGINE_TREE => match path.get(1) {
                Some((_, Index::Node(node)))
                    if !path.iter().skip(2).any(|(_, index)| matches!(index, Index::Node(_))) =>
                {
                    touched.tree_nodes.insert(*node);
                }
                _ => return Err(FallbackReason::Unclassified),
            },
            META | TREE => return Err(FallbackReason::GlobalStructure),
            ALIASES if !through_node => touched.aliases = true,
            // The navigator order and the title never reach a limit; any other root is not read.
            _ if !through_node => {}
            // A tree node on the path of a container under `doc`, the alias map or any other root.
            _ => return Err(FallbackReason::Unclassified),
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
fn finish(
    facts: &BaseFacts,
    work_added: u64,
    mark_growth: u64,
    delta: &Delta,
    limits: &DocumentLimits,
) -> Result<WindowReport, Undecided> {
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
    // The items of the page after the import: the base's plus what the update's ops can have added,
    // against the budget of the exact totals just computed (`crate::work`).
    if !injected!(WorkBudgetSkipped)
        && facts.items.saturating_add(work_added).saturating_add(mark_growth)
            > crate::work::item_budget_with_marks(
                blocks as u64,
                navigators as u64,
                chars as u64,
                facts.mark_items.saturating_add(mark_growth),
            )
    {
        return Err(FallbackReason::WorkBudget.into());
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod gate_tests {
    use std::time::{Duration, Instant};

    use loro::{ID, VersionVector};

    use super::{ChangeSpan, OpSummary};
    use crate::facts::BaseVersion;

    fn base(heads: u64) -> (BaseVersion, VersionVector) {
        let mut vv = VersionVector::new();
        let frontier: Vec<ID> = (0..heads)
            .map(|peer| {
                vv.insert(peer, 1);
                ID::new(peer, 0)
            })
            .collect();
        (
            BaseVersion {
                oplog_vv: vv.clone(),
                frontier,
                max_lamport: Some(10),
            },
            vv,
        )
    }

    fn summary(changes: Vec<ChangeSpan>) -> OpSummary {
        OpSummary {
            changes,
            ..OpSummary::default()
        }
    }

    /// R3-5: a change that depends on every one of 100,000 heads is decided in linear time (the
    /// quadratic scan compared about 10^10 ids).
    #[test]
    fn a_change_that_depends_on_every_head_of_a_base_with_many_heads_is_cheap() {
        let (version, vv) = base(100_000);
        let deps = version.frontier.clone();
        let sequential = summary(vec![ChangeSpan {
            peer: 1_000_000,
            start: 0,
            end: 1,
            lamport: 11,
            deps,
        }]);
        let started = Instant::now();
        assert!(sequential.strictly_sequential(&version, &vv));
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
    }

    /// R3-5: 100,000 one-op changes of one peer, each depending on the previous one, are decided in
    /// time that does not grow with the square of the changes.
    #[test]
    fn a_long_chain_of_changes_of_one_peer_is_cheap() {
        let (version, vv) = base(1);
        let first_dep = version.frontier.clone();
        let changes: Vec<ChangeSpan> = (0..100_000)
            .map(|counter| ChangeSpan {
                peer: 1_000_000,
                start: counter,
                end: counter + 1,
                lamport: 11 + counter.unsigned_abs(),
                deps: if counter == 0 {
                    first_dep.clone()
                } else {
                    vec![ID::new(1_000_000, counter - 1)]
                },
            })
            .collect();
        let chain = summary(changes);
        let started = Instant::now();
        assert!(chain.strictly_sequential(&version, &vv));
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
    }
}
