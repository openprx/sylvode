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
//! Incremental evaluation only ever *accepts*. Anything else, whether a limit is reached, a unit is
//! invalid, a structure changes or the check cannot classify what the update did with certainty,
//! returns a [`FallbackReason`] and the caller runs today's full computation (`semantic_snapshot`
//! and `check_snapshot`), so every refusal carries exactly the reason, limit kind and numbers it
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
//! Everything whose effect on other units the check does not model falls back: any op on the `doc`
//! map, the structure meta or the structure tree; a non-map element inserted into the list; any
//! move or delete in the legacy tree and any create that is not what its id says; a tree node that gets its id after the fact, or two nodes with one
//! id; a unit whose key collides across the two kinds (a tree id that looks like a container id);
//! and a base the facts do not describe.

use std::collections::{HashMap, HashSet};

use loro::{Container, ContainerID, ContainerType, Index, JsonListOp, JsonOp, JsonOpContent, JsonTreeOp, TreeID};

use crate::engine::{ImportTrace, LoroCollabEngine, TreeUnit};
use crate::error::CollabError;
use crate::facts::{BaseFacts, Contribution, UnitFact};
use crate::limits::{DocumentLimits, check_snapshot};
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

/// Why the incremental check did not decide, so that the full computation did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
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
    /// An existing tree node got a logical id from this update, or the id caches were rebuilt in
    /// full.
    ResolvedNodeId,
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
    /// A limit is reached: the full computation reports it.
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
        Err(reason) => {
            #[cfg(test)]
            crate::walk_counter::record_fallback();
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
            ContainerID::Normal { .. } if Some(&op.container) == root_list => self.visit_root_list(op),
            ContainerID::Normal { .. } => {
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
            // The navigator order and the title never reach a limit; any other root is not read.
            ENGINE_TREE => match &op.content {
                JsonOpContent::Tree(JsonTreeOp::Create { target, .. }) if *target == TreeID::new(peer, op.counter) => {
                    self.tree_created.push(*target);
                }
                _ => self.seen.set(Seen::TREE_OTHER),
            },
            _ => {}
        }
    }

    fn visit_root_list(&mut self, op: &JsonOp) {
        match &op.content {
            JsonOpContent::List(JsonListOp::Insert { value, .. }) => {
                for element in value {
                    match element {
                        loro::LoroValue::Container(id @ ContainerID::Normal { container_type, .. })
                            if *container_type == ContainerType::Map =>
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

/// The units an update touched, mapped from its ops.
#[derive(Debug, Default)]
struct Touched {
    blocks: HashSet<ContainerID>,
    tree_nodes: HashSet<TreeID>,
    aliases: bool,
}

const CID_PREFIX: &str = "cid:";

/// `Ok(report)` when the update is certainly accepted by the full computation too, and the reason
/// the check did not decide otherwise.
fn check(
    engine: &LoroCollabEngine,
    facts: Option<&BaseFacts>,
    trace: &ImportTrace,
) -> Result<WindowReport, FallbackReason> {
    let facts = facts.ok_or(FallbackReason::FactsAbsent)?;
    // A causal import never leaves anything pending; if it did, the ops seen are not the update.
    if trace.status.pending.is_some() {
        return Err(FallbackReason::Unclassified);
    }
    if facts.state != trace.before_state {
        return Err(FallbackReason::StaleFacts);
    }
    let summary = &trace.summary;
    if !injected!(StructuralFallbackSkipped) {
        if summary.seen.has(Seen::GLOBAL) {
            return Err(FallbackReason::GlobalStructure);
        }
        if summary.seen.has(Seen::TREE_OTHER) {
            return Err(FallbackReason::TreeStructure);
        }
        if engine.has_duplicate_logical_ids() {
            return Err(FallbackReason::DuplicateLogicalId);
        }
        if trace.resolved_existing_id {
            return Err(FallbackReason::ResolvedNodeId);
        }
    }
    if summary.seen.has(Seen::UNREADABLE_INSERT) {
        return Err(FallbackReason::UnreadableInsert);
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

    let mut delta = Delta::default();
    for id in removed.iter().filter(|_| !injected!(RemovedBlocksIgnored)) {
        // A block that left the list contributes nothing now. A tree node with its container id
        // as logical id would be dropped by the projection beside it: not modelled.
        let key = id.to_string();
        if engine.has_logical_id(&key) {
            return Err(FallbackReason::KeyCollision);
        }
        delta.add(None, facts.units.get(key.as_str()));
    }
    evaluate_blocks(engine, facts, &touched, &mut delta)?;
    evaluate_tree_nodes(engine, facts, summary, &touched, &mut delta)?;
    finish(facts, &delta)
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
            _ => {}
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
    summary: &OpSummary,
    touched: &Touched,
    delta: &mut Delta,
) -> Result<(), FallbackReason> {
    let created: HashSet<TreeID> = summary.tree_created.iter().copied().collect();
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
fn finish(facts: &BaseFacts, delta: &Delta) -> Result<WindowReport, FallbackReason> {
    let limits = DocumentLimits::default();
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
    if delta.max_depth > limits.tree_depth_max
        || navigators > limits.container_count_max
        || blocks > limits.document_block_count_max
        || delta.longest > limits.text_block_chars_max
        || chars > limits.document_text_chars_max
    {
        return Err(FallbackReason::LimitReached);
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
    let deepest = projection
        .nodes
        .keys()
        .map(|id| crate::limits::depth_of(projection, id))
        .max()
        .unwrap_or(0) as u64;
    if deepest > report.depth_bound {
        return Err(WindowDivergence {
            field: "depth_bound",
            reported: report.depth_bound,
            projected: deepest,
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
    if let Err(violation) = check_snapshot(projection, &DocumentLimits::default()) {
        return Err(WindowDivergence {
            field: violation.limit_kind,
            reported: violation.limit,
            projected: violation.observed,
        });
    }
    Ok(())
}
