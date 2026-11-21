//! The Loro adapter selected in v0.3, promoted from `spikes/collab-loro` to production.
//!
//! Every business-facing id in this module is a [`NodeId`] chosen by the caller (the Flow command
//! layer, or a fixture in this crate's own tests). Loro's own [`TreeID`] (which embeds a
//! [`loro::PeerID`]) never crosses out of this file: it is looked up from a purely local,
//! non-synced cache (`id_to_tree` / `tree_to_id`), rebuilt after every `load`/`import_update` by
//! reading back a `logical_id` field this adapter itself writes into each tree node's meta map.
//! That `logical_id` field is business data flowing *into* the engine (so this replica can find
//! "the node the fixture calls `blk-abc123`" again after a remote peer creates it) — it is not the
//! engine exposing its own identity outward.
//!
//! Beyond the `tree` container the spike adapter had, this production version also attaches a
//! root-level `meta` map for document-level fields. `ADR-0002` (`decisions/` in the Sylvode
//! Flow contracts repository) freezes `meta.title` as the canonical source for a Page/Collection
//! title (`flow_object_projections.title` is only the rebuildable read replica), so the engine
//! needs a place to hold it that survives snapshot/update round trips exactly like tree content
//! does.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::OnceLock;

use loro::{
    Container, Frontiers, LoroDoc, LoroMap, LoroText, LoroTree, LoroValue, TreeID, TreeParentId, ValueOrContainer,
    VersionVector,
};
use serde_json::{Map as JsonMap, Number, Value};

use crate::error::{CollabError, InputLimits};
use crate::frontier::Frontier;
use crate::operation::{NodeId, NodeKind, Operation};
use crate::semantic::{SemanticNode, SemanticSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineMetadata {
    pub candidate: &'static str,
    pub rust_engine: &'static str,
    pub rust_engine_version: &'static str,
}

#[must_use]
pub const fn metadata() -> EngineMetadata {
    EngineMetadata {
        candidate: "loro",
        rust_engine: "loro",
        rust_engine_version: "1.18.0",
    }
}

/// Summary of what an `import_update` call actually changed, without exposing any engine-internal
/// op/diff representation to callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Diff {
    /// `false` when the update was a byte-for-byte or semantic no-op (e.g. a duplicate replay).
    pub changed: bool,
}

/// The frozen v0.3 sync contract every adapter implements:
///
/// ```ignore
/// trait CollabEngine {
///     fn load(snapshot: &[u8]) -> Result<Self>;
///     fn import_update(&mut self, update: &[u8]) -> Result<Diff>;
///     fn export_snapshot(&self) -> Result<Vec<u8>>;
///     fn export_from(&self, frontier: &Frontier) -> Result<Vec<u8>>;
///     fn frontier(&self) -> Frontier;
/// }
/// ```
///
/// Every method returns a typed [`CollabError`] (never a boxed/opaque error), every byte-slice
/// input is length-validated before it reaches the underlying engine's decoder (see
/// [`LoroCollabEngine`]'s use of [`InputLimits`]), and [`Frontier`] is an opaque byte wrapper so no
/// implementation can leak an engine-specific peer/client id through this trait.
pub trait CollabEngine: Sized {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Loads a document from a full snapshot previously produced by [`Self::export_snapshot`].
    fn load(snapshot: &[u8]) -> Result<Self, Self::Error>;

    /// Applies a remote update (full snapshot or incremental delta) produced by
    /// [`Self::export_snapshot`] or [`Self::export_from`]. Must be atomic: on error, local state
    /// (and therefore [`Self::frontier`]) is left byte-for-byte unchanged.
    fn import_update(&mut self, update: &[u8]) -> Result<Diff, Self::Error>;

    /// Exports the full document state (history + current state) as a single portable blob.
    fn export_snapshot(&self) -> Result<Vec<u8>, Self::Error>;

    /// Exports only the changes this replica has that the given remote `frontier` does not.
    fn export_from(&self, frontier: &Frontier) -> Result<Vec<u8>, Self::Error>;

    /// The current version marker for this replica, suitable for passing to a peer's
    /// [`Self::export_from`].
    fn frontier(&self) -> Frontier;
}

const TREE_CONTAINER: &str = "tree";
const META_CONTAINER: &str = "meta";
const META_TITLE: &str = "title";
const META_LOGICAL_ID: &str = "logical_id";
const META_KIND: &str = "kind";
const META_TEXT: &str = "text";
const META_PROPERTY_PREFIX: &str = "prop:";

/// The containers [`LoroCollabEngine::prewarm_parts`] materialises, as a set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PrewarmParts(u8);

impl PrewarmParts {
    pub(crate) const BLOCK_LIST: Self = Self(1);
    pub(crate) const ALIASES: Self = Self(2);
    pub(crate) const ORDER: Self = Self(4);
    pub(crate) const STRUCTURE_TREE: Self = Self(8);
    pub(crate) const ALL: Self = Self(15);
    #[cfg(test)]
    pub(crate) const NONE: Self = Self(0);

    const fn has(self, part: Self) -> bool {
        self.0 & part.0 != 0
    }
}

pub struct LoroCollabEngine {
    doc: LoroDoc,
    tree: LoroTree,
    meta: LoroMap,
    id_to_tree: HashMap<NodeId, TreeID>,
    tree_to_id: HashMap<TreeID, NodeId>,
    /// Tree nodes the last rebuild saw without a readable `logical_id`. A rebuild retries them, as
    /// the full rebuild retries every uncached node, since an import can still give them one.
    uncached_tree_nodes: HashSet<TreeID>,
    /// Two live tree nodes have claimed one `logical_id`. Whichever the cache keeps, the full
    /// projection keeps one node under that id; the incremental check does not model that.
    duplicate_logical_ids: bool,
    /// The last import rebuilt the id caches from its own ops (`rebuild_id_cache_for`) instead of
    /// in full. Only a strictly sequential import does; [`Self::complete_id_cache`] makes the
    /// caches exactly the full rebuild's before the full computation reads them.
    id_cache_incremental: bool,
    /// Memoized [`LoroCollabEngine::minimum_editor_model`] result for the current document state.
    ///
    /// Every method that can change the document takes `&mut self` and clears this cell first
    /// (see [`LoroCollabEngine::invalidate_editor_model_floor`]); no `&self` method writes to the
    /// document. As a second, independent guard the entry is keyed by the state frontier it was
    /// computed at and is used only while that frontier is current and no local transaction is
    /// pending. Errors are never cached, so a malformed document is re-walked and re-rejected
    /// with the same typed error every time.
    editor_model_floor: OnceLock<CachedEditorModelFloor>,
    /// The editor session's placement index (see [`crate::editor_structure::PlacementIndex`]),
    /// built on the first structural edit of a command build and updated by each one after it.
    ///
    /// It is used only while `session_stamp` equals the document's state frontier and no local
    /// transaction is pending, so any change the session did not make itself (an import, a revert,
    /// an atom insert, ...) retires it; every such method also drops it explicitly. An operation
    /// that fails drops it too.
    session_index: Option<crate::editor_structure::PlacementIndex>,
    session_stamp: Option<Frontiers>,
    /// Test only: route structural and text edits through the recompute-every-time reference.
    #[cfg(test)]
    recompute_placements: bool,
}

#[derive(Debug)]
struct CachedEditorModelFloor {
    state: Frontiers,
    floor: u32,
}

const fn kind_to_str(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Block => "block",
        NodeKind::CollectionField => "collection_field",
        NodeKind::CollectionView => "collection_view",
        NodeKind::RecordProperty => "record_property",
        NodeKind::NavigatorNode => "navigator_node",
    }
}

fn str_to_kind(value: &str) -> NodeKind {
    match value {
        "collection_field" => NodeKind::CollectionField,
        "collection_view" => NodeKind::CollectionView,
        "record_property" => NodeKind::RecordProperty,
        "navigator_node" => NodeKind::NavigatorNode,
        _ => NodeKind::Block,
    }
}

fn map_loro_error(err: &loro::LoroError, node_id: &str) -> CollabError {
    if matches!(err, loro::LoroError::TreeError(loro::LoroTreeError::CyclicMoveError)) {
        CollabError::CycleRejected {
            id: node_id.to_string(),
        }
    } else {
        CollabError::OperationFailed {
            reason: err.to_string(),
        }
    }
}

/// Zero-pads `position` to at least 8 digits (matching `format!("{position:08}")`'s exact output
/// for every non-negative value, including the unpadded case where `position` already has 8+
/// digits), without routing through `core::fmt`'s generic width/fill machinery
/// (`Formatter::pad_integral`).
///
/// `semantic_snapshot` calls this once per node, so its cost multiplies by document size; a
/// release-mode `perf` profile of the isolated-apply worker at the `container_count_max`/
/// `document_block_count_max` boundary (10,000 nodes) showed `Formatter::pad_integral` and the
/// `core::fmt::write` machinery it pulls in accounting for roughly a tenth of total samples,
/// almost entirely attributable to this one call site -- the `{:08}` width specifier routes
/// unsigned integer formatting through that generic, allocating-per-`write_str`-call path instead
/// of `usize::to_string`'s specialized fast path. Using `to_string` (fast path) plus a manual
/// left-pad keeps the identical output with meaningfully less per-call overhead.
fn zero_padded_order_key(position: usize) -> String {
    let digits = position.to_string();
    if digits.len() >= 8 {
        return digits;
    }
    let mut key = String::with_capacity(8);
    for _ in 0..(8 - digits.len()) {
        key.push('0');
    }
    key.push_str(&digits);
    key
}

fn loro_value_to_json(value: &LoroValue) -> Value {
    match value {
        LoroValue::Null | LoroValue::Binary(_) | LoroValue::Container(_) => Value::Null,
        LoroValue::Bool(flag) => Value::Bool(*flag),
        LoroValue::Double(number) => Number::from_f64(*number).map_or(Value::Null, Value::Number),
        LoroValue::I64(number) => Value::Number((*number).into()),
        LoroValue::String(text) => Value::String(text.to_string()),
        LoroValue::List(items) => Value::Array(items.iter().map(loro_value_to_json).collect()),
        LoroValue::Map(entries) => {
            let mut object = JsonMap::new();
            for (key, child) in entries.iter() {
                object.insert(key.clone(), loro_value_to_json(child));
            }
            Value::Object(object)
        }
    }
}

/// The model-5 marker (`flow_editor_structure_meta.model == "rich_page_v5"`), read by typed
/// container id. The read registers nothing, on a legacy document or any other: the exported
/// value, the snapshot bytes and the list of roots are the same afterwards.
///
/// # Errors
/// A failed root read (`editor_root_probe_failed`).
pub(crate) fn model5_marker(doc: &LoroDoc) -> Result<bool, CollabError> {
    Ok(matches!(
        crate::editor_structure::reserved_root(doc, crate::reserved_roots::META)?,
        Some(Container::Map(meta))
            if matches!(meta.get("model"), Some(ValueOrContainer::Value(LoroValue::String(model)))
                if model.as_ref() == "rich_page_v5")
    ))
}

/// Whether some physical block map in `doc.children` has an atom `nodeName`, placed or not.
/// `Ok(false)` without an editor document.
pub(crate) fn physical_atoms_present(doc: &LoroDoc) -> Result<bool, CollabError> {
    if !editor_doc_present(doc)? {
        return Ok(false);
    }
    Ok(crate::editor_structure::physical(doc)?.iter().any(|map| {
        matches!(
            map.get("nodeName"),
            Some(ValueOrContainer::Value(LoroValue::String(kind))) if crate::editor_media::is_atom(kind.as_ref())
        )
    }))
}

/// Whether the `doc` root map holds a `children` list (browser prose owns the document).
fn editor_doc_present(doc: &LoroDoc) -> Result<bool, CollabError> {
    Ok(
        crate::editor_structure::root_map(doc, crate::reserved_roots::DOC)?.is_some_and(|root| {
            matches!(
                root.get("children"),
                Some(ValueOrContainer::Container(Container::List(_)))
            )
        }),
    )
}

fn read_string_value(value_or_container: &ValueOrContainer) -> Option<String> {
    match value_or_container {
        ValueOrContainer::Value(LoroValue::String(s)) => Some(s.as_ref().to_string()),
        _ => None,
    }
}

/// The result of counting the items of a page's projection (see `LoroCollabEngine::projection_items`).
#[derive(Debug)]
pub(crate) enum ProjectionCount {
    /// The count finished: the items of the page and the mark items of every block that has any.
    Within { items: u64, marks: HashMap<NodeId, u64> },
    /// The items passed the ceiling.
    OverCeiling,
    /// A setup guard fired (`crate::setup_guard`).
    GuardFired(crate::setup_guard::Fired),
}

impl LoroCollabEngine {
    fn attach_tree(doc: &LoroDoc) -> LoroTree {
        let tree = doc.get_tree(TREE_CONTAINER);
        // jitter=0 keeps sibling ordering fully deterministic for a given operation log; Loro
        // still resolves *concurrent* insert/move collisions correctly (this is exactly what the
        // moveable-tree CRDT is for).
        tree.enable_fractional_index(0);
        tree
    }

    fn attach_meta(doc: &LoroDoc) -> LoroMap {
        doc.get_map(META_CONTAINER)
    }

    fn resolve_parent(&self, parent: Option<&NodeId>) -> Result<TreeParentId, CollabError> {
        parent.map_or(Ok(TreeParentId::Root), |id| {
            self.id_to_tree
                .get(id)
                .copied()
                .map(TreeParentId::Node)
                .ok_or_else(|| CollabError::UnknownNode { id: id.to_string() })
        })
    }

    fn require_tree_id(&self, id: &NodeId) -> Result<TreeID, CollabError> {
        self.id_to_tree
            .get(id)
            .copied()
            .ok_or_else(|| CollabError::UnknownNode { id: id.to_string() })
    }

    /// Reads back every tree node's `logical_id` meta field to (re)populate the local id caches.
    /// Called after construction and after every successful `load`/`import_update`, since remote
    /// peers may have created nodes this replica has never locally indexed before.
    ///
    /// Known limitation, older than the incremental window: a node the caches already hold is never
    /// read again, so an update that rewrites (or deletes) the `logical_id` of an existing node
    /// leaves the caches, and with them the projection of this engine, on the old id, while an
    /// engine loaded from the exported snapshot reads the new one. Both engines decide a limit on
    /// what they see, so the same update can have a different projection in the worker than on the
    /// host. The window with base facts (`[flow] incremental_window` on) refuses such an update
    /// before any decision (`window_check::ID_REWRITE_REASON`); with the switch off the write is
    /// `next`'s, and the host refuses a result its projection finds over a limit.
    fn rebuild_id_cache(&mut self) -> Result<(), CollabError> {
        for tree_id in self.tree.nodes() {
            if self.tree_to_id.contains_key(&tree_id) {
                continue;
            }
            self.cache_tree_node(tree_id)?;
        }
        Ok(())
    }

    /// Makes the id caches what the full rebuild gives, if the last import built them from its own
    /// ops. The full computation after a fallback reads the caches, and it must see exactly what
    /// `next`'s full rebuild would have given it: the rebuild skips nodes that are cached, the
    /// incremental one cached only nodes it read, so the result is the same (duplicate claims go to
    /// the full rebuild before they are cached), and a node no op of the import named, which the
    /// incremental rebuild cannot know, is named here.
    ///
    /// # Errors
    /// A failed metadata read, which the full rebuild reports as well.
    pub(crate) fn complete_id_cache(&mut self) -> Result<(), CollabError> {
        if self.id_cache_incremental {
            self.rebuild_id_cache()?;
            self.id_cache_incremental = false;
        }
        Ok(())
    }

    /// [`Self::rebuild_id_cache`] for a strictly sequential import
    /// ([`crate::window_check::OpSummary::strictly_sequential`]) whose ops are known: `created`
    /// holds every tree node that the imported ops created. Such an import adds no node that no op
    /// of it names (see the module documentation of `window_check`), and gives no existing node an
    /// id without a write of its `logical_id` (which the check turns into a fallback), so the only
    /// nodes to read are the new ones. Cost: O(created) instead of O(tree). Never used for any other import: the library can materialise nodes the base's
    /// history holds but its state dropped when an update is concurrent with the base.
    ///
    /// Returns whether one of the created targets was a node the tree already held (or the full
    /// rebuild decided instead, because two nodes claim one id): an existing node that a create
    /// "creates" again keeps its subtree, which the incremental check does not model.
    ///
    /// The two caches together hold every node of the tree before the import, live or deleted,
    /// named or not: the load-time rebuild visits all of `LoroTree::nodes()`, which includes the
    /// deleted ones, and every later rebuild adds what it visits. A created target in either of
    /// them is therefore not new, whatever the ops that created it look like (a hand-encoded move
    /// of a target whose create op the base never had makes the node exist without it).
    fn rebuild_id_cache_for(&mut self, created: &[TreeID]) -> Result<bool, CollabError> {
        let created_now: HashSet<TreeID> = created.iter().copied().collect();
        #[cfg(test)]
        let checked =
            crate::window_check::fault::active() != Some(crate::window_check::fault::Fault::CreatedTargetChecksSkipped);
        #[cfg(not(test))]
        let checked = true;
        if checked
            && created
                .iter()
                .any(|id| self.tree_to_id.contains_key(id) || self.uncached_tree_nodes.contains(id))
        {
            // The full rebuild decides, as for two claims of one id.
            self.rebuild_id_cache()?;
            return Ok(true);
        }
        // Only the created nodes are read. An uncached node (one a previous rebuild found without
        // a `logical_id`) can gain an id only through a write of the `logical_id` key on its
        // metadata map. With base facts the window refuses that write before any decision
        // (`ID_REWRITE_REASON`); without facts the import is not sequential and the full rebuild
        // runs; any other map's `logical_id` falls back (`IdRewritten`), after which
        // `complete_id_cache` names the node. Reading uncached nodes here would make the window
        // O(page).
        // Read first, change nothing: if two nodes would claim one id, the full rebuild decides,
        // since which of them the cache keeps depends on the order they are read in (tree order).
        let mut reads: Vec<(TreeID, Option<NodeId>)> = Vec::with_capacity(created_now.len());
        for tree_id in created_now {
            if self.tree_to_id.contains_key(&tree_id) || !self.tree.contains(tree_id) {
                continue;
            }
            reads.push((tree_id, self.read_logical_id(tree_id)?));
        }
        let mut claimed: HashSet<&NodeId> = HashSet::with_capacity(reads.len());
        let duplicate = reads
            .iter()
            .filter_map(|(_, id)| id.as_ref())
            .any(|id| self.id_to_tree.contains_key(id) || !claimed.insert(id));
        if duplicate {
            self.rebuild_id_cache()?;
            return Ok(true);
        }
        for (tree_id, logical) in reads {
            self.store_logical_id(tree_id, logical);
        }
        Ok(false)
    }

    /// Reads one tree node's `logical_id` into the caches, or remembers that it has none yet.
    fn cache_tree_node(&mut self, tree_id: TreeID) -> Result<(), CollabError> {
        let logical = self.read_logical_id(tree_id)?;
        self.store_logical_id(tree_id, logical);
        Ok(())
    }

    /// A tree node's `logical_id`, if it has a readable one.
    fn read_logical_id(&self, tree_id: TreeID) -> Result<Option<NodeId>, CollabError> {
        #[cfg(test)]
        crate::walk_counter::record_id_read();
        let meta = self
            .tree
            .get_meta(tree_id)
            .map_err(|e| map_loro_error(&e, "<rebuild>"))?;
        Ok(meta
            .get(META_LOGICAL_ID)
            .and_then(|raw| read_string_value(&raw))
            .map(NodeId::from))
    }

    /// Caches a node's logical id, or remembers that it has none yet.
    fn store_logical_id(&mut self, tree_id: TreeID, logical: Option<NodeId>) {
        let Some(id) = logical else {
            self.uncached_tree_nodes.insert(tree_id);
            return;
        };
        if self
            .id_to_tree
            .insert(id.clone(), tree_id)
            .is_some_and(|previous| previous != tree_id)
        {
            self.duplicate_logical_ids = true;
        }
        self.tree_to_id.insert(tree_id, id);
        self.uncached_tree_nodes.remove(&tree_id);
    }

    fn text_handler(&self, tree_id: TreeID) -> Result<LoroText, CollabError> {
        let meta = self.tree.get_meta(tree_id).map_err(|e| map_loro_error(&e, "<text>"))?;
        meta.ensure_mergeable_text(META_TEXT)
            .map_err(|e| map_loro_error(&e, "<text>"))
    }

    /// A caller-supplied index is rolled independently of the actual sibling count at apply time
    /// (deliberately: a client's intended insert position can be stale by the time the op applies
    /// against the server's canonical head), so clamping to the valid range here — rather than
    /// requiring the caller to somehow predict it — mirrors realistic adapter behavior.
    fn clamp_index(&self, parent: TreeParentId, requested: u32) -> usize {
        let children_count = self.tree.children_num(parent).unwrap_or(0);
        (requested as usize).min(children_count)
    }

    /// A deep, independent copy of this engine (Loro's `LoroDoc::fork`, not `Clone`: `Clone` on a
    /// `LoroDoc` is a *reference* clone that shares the same underlying document, which would let
    /// a caller that mutates the copy also mutate `self`).
    ///
    /// Exists so a caller can apply a candidate update to an isolated working copy without
    /// touching the original — the shape the v0.4 collab server's warm cache needs (`ADR-0010`:
    /// hydrate/isolated-apply happen outside any lock and outside the shared cache entry; only a
    /// *successful, committed* write is allowed to replace it).
    ///
    /// # Errors
    /// Propagates [`Self::rebuild_id_cache`]'s failure mode, which cannot happen for a `self` that
    /// was itself produced by `new_empty`/`load`/`apply_operation`/`import_update` (i.e. every
    /// engine this crate can hand a caller), but is surfaced rather than assumed away.
    pub fn fork(&self) -> Result<Self, CollabError> {
        let doc = self.doc.fork();
        crate::page_inline::configure(&doc);
        let tree = Self::attach_tree(&doc);
        let meta = Self::attach_meta(&doc);
        let mut engine = Self {
            doc,
            tree,
            meta,
            id_to_tree: HashMap::new(),
            tree_to_id: HashMap::new(),
            uncached_tree_nodes: HashSet::new(),
            duplicate_logical_ids: false,
            id_cache_incremental: false,
            editor_model_floor: OnceLock::new(),
            session_index: None,
            session_stamp: None,
            #[cfg(test)]
            recompute_placements: false,
        };
        engine.rebuild_id_cache()?;
        Ok(engine)
    }

    /// Creates a fresh, empty document (no blocks, no title). `replica_seed` deterministically
    /// seeds the engine's internal peer id.
    #[must_use]
    pub fn new_empty(replica_seed: u64) -> Self {
        let doc = LoroDoc::new();
        crate::page_inline::configure(&doc);
        // PeerID 0 is reserved by loro's internals for some sentinel comparisons; keep every
        // replica's seed in the non-zero range while staying deterministic.
        let peer_id = replica_seed.wrapping_add(1).max(1);
        // A brand-new, uncommitted doc always accepts a peer id change.
        let _ = doc.set_peer_id(peer_id);
        let tree = Self::attach_tree(&doc);
        let meta = Self::attach_meta(&doc);
        Self {
            doc,
            tree,
            meta,
            id_to_tree: HashMap::new(),
            tree_to_id: HashMap::new(),
            uncached_tree_nodes: HashSet::new(),
            duplicate_logical_ids: false,
            id_cache_incremental: false,
            editor_model_floor: OnceLock::new(),
            session_index: None,
            session_stamp: None,
            #[cfg(test)]
            recompute_placements: false,
        }
    }

    /// The document-level title (`ADR-0002`: `meta.title` is canonical; `flow_object_projections`
    /// only mirrors it). Empty when no title has ever been set.
    ///
    /// # Errors
    /// Only if the underlying engine reports a read failure on an attached map, which does not
    /// happen for a well-formed local document.
    pub fn title(&self) -> Result<String, CollabError> {
        Ok(self
            .meta
            .get(META_TITLE)
            .and_then(|value| read_string_value(&value))
            .unwrap_or_default())
    }

    /// Validates an engine-derived version against a persisted Loro wire vector.
    ///
    /// Loro 1.18 encoded its peer map in hash-map iteration order, so frontiers persisted before
    /// [`CollabEngine::frontier`] became canonical may list the same entries in any order. The
    /// comparison is therefore semantic. A wire value that does not decode never matches.
    pub fn matches_frontier(&self, wire: &[u8]) -> bool {
        decode_frontier_entries(wire).is_ok_and(|entries| entries == state_entries(&self.doc.state_vv()))
    }

    /// Whether two persisted or client-supplied frontiers name the same causal version.
    ///
    /// Frontiers are compared as decoded version vectors, never as bytes: the same version may be
    /// stored in canonical (peer-sorted) order or in the iteration order older releases emitted.
    /// The empty byte string is the empty version, matching [`CollabEngine::export_from`].
    ///
    /// # Errors
    /// `DecodeFailed` (`input: "frontier"`) when either side is not a well-formed, minimally
    /// encoded version vector without duplicate peers or trailing bytes. An undecodable frontier
    /// is never reported as equal to anything.
    pub fn frontiers_equivalent(left: &[u8], right: &[u8]) -> Result<bool, CollabError> {
        let left_entries = decode_frontier_entries(left)?;
        let right_entries = decode_frontier_entries(right)?;
        Ok(left_entries == right_entries)
    }

    /// Re-encodes a persisted or client-supplied frontier in canonical (peer-sorted) form.
    ///
    /// # Errors
    /// `DecodeFailed` under the same conditions as [`Self::frontiers_equivalent`].
    pub fn canonical_frontier(wire: &[u8]) -> Result<Frontier, CollabError> {
        if wire.is_empty() {
            return Ok(Frontier::from_bytes(Vec::new()));
        }
        Ok(Frontier::from_bytes(encode_frontier_entries(&decode_frontier_entries(
            wire,
        )?)))
    }

    /// Creates an independent read-only historical view at an opaque version-vector frontier.
    ///
    /// Loro full snapshots retain operation history, so this remains valid after the server has
    /// advanced its canonical snapshot pointer. The returned engine contains only history up to
    /// `frontier`; callers cannot accidentally expose Loro peer ids because the input and output
    /// boundary remains [`Frontier`] plus the semantic accessors on this type.
    ///
    /// # Errors
    /// `DecodeFailed` when `frontier` is not an encoded version vector, or `OperationFailed` when
    /// it is not represented by this document's retained history.
    pub fn fork_at_frontier(&self, frontier: &Frontier) -> Result<Self, CollabError> {
        let vv = if frontier.is_empty() {
            VersionVector::default()
        } else {
            VersionVector::decode(frontier.as_bytes()).map_err(|error| CollabError::DecodeFailed {
                input: "frontier",
                reason: error.to_string(),
            })?
        };
        let frontiers = self.doc.vv_to_frontiers(&vv);
        let doc = self
            .doc
            .fork_at(&frontiers)
            .map_err(|error| CollabError::OperationFailed {
                reason: error.to_string(),
            })?;
        // SnapshotAt may omit metadata of deleted placement nodes, while the immutable
        // physical carriers remain in the editor list. Replay that exact causal range to
        // retain tombstones required by the canonical structure validator.
        let doc = match crate::editor_structure::placements(&doc) {
            Err(CollabError::OperationFailed { reason }) if reason == "editor_structure_unindexed_physical_block" => {
                let spans = vv
                    .iter()
                    .map(|(&peer, &end)| loro::IdSpan::new(peer, 0, end))
                    .collect::<Vec<_>>();
                let updates = self
                    .doc
                    .export(loro::ExportMode::updates_in_range(spans))
                    .map_err(|error| CollabError::OperationFailed {
                        reason: error.to_string(),
                    })?;
                InputLimits::default().validate_snapshot(&updates)?;
                let mut historical = Self::new_empty(0);
                historical
                    .doc
                    .import(&updates)
                    .map_err(|error| CollabError::DecodeFailed {
                        input: "historical operations",
                        reason: error.to_string(),
                    })?;
                historical.rebuild_id_cache()?;
                if historical.doc.state_vv() != vv || historical.doc.oplog_vv() != vv {
                    return Err(CollabError::OperationFailed {
                        reason: "historical frontier not retained".into(),
                    });
                }
                crate::editor_structure::placements(&historical.doc)?;
                return Ok(historical);
            }
            Err(error) => return Err(error),
            Ok(_) => doc,
        };
        if doc.oplog_vv() != vv {
            return Err(CollabError::OperationFailed {
                reason: "historical frontier not retained".into(),
            });
        }
        let tree = Self::attach_tree(&doc);
        let meta = Self::attach_meta(&doc);
        let mut fork = Self {
            doc,
            tree,
            meta,
            id_to_tree: HashMap::new(),
            tree_to_id: HashMap::new(),
            uncached_tree_nodes: HashSet::new(),
            duplicate_logical_ids: false,
            id_cache_incremental: false,
            editor_model_floor: OnceLock::new(),
            session_index: None,
            session_stamp: None,
            #[cfg(test)]
            recompute_placements: false,
        };
        fork.rebuild_id_cache()?;
        Ok(fork)
    }

    /// Reverts the visible document to `frontier` by appending inverse operations.
    ///
    /// Loro's `revert_to` records a new local change on the current head. History at `frontier`
    /// stays reachable through [`Self::fork_at_frontier`], and a peer that already moved past
    /// `frontier` imports the exported delta instead of rewinding its log.
    ///
    /// # Errors
    /// `DecodeFailed` when `frontier` is not an encoded version vector, or `OperationFailed`
    /// when that version is not in this document's retained history.
    pub fn revert_to_frontier(&mut self, frontier: &Frontier) -> Result<(), CollabError> {
        let prior_model = self.minimum_editor_model()?;
        self.invalidate_editor_model_floor();
        let vv = if frontier.is_empty() {
            VersionVector::default()
        } else {
            VersionVector::decode(frontier.as_bytes()).map_err(|error| CollabError::DecodeFailed {
                input: "frontier",
                reason: error.to_string(),
            })?
        };
        let frontiers = self.doc.vv_to_frontiers(&vv);
        self.doc
            .revert_to(&frontiers)
            .map_err(|error| CollabError::OperationFailed {
                reason: error.to_string(),
            })?;
        if prior_model >= 5 {
            self.doc
                .get_map(crate::editor_structure::META)
                .insert("model", "rich_page_v5")
                .map_err(|error| CollabError::OperationFailed {
                    reason: error.to_string(),
                })?;
        }
        self.doc.commit();
        self.rebuild_id_cache()?;
        Ok(())
    }

    /// The whole document state as JSON, for callers that need the editor container as well as
    /// the semantic tree. Binary values, non-finite numbers, and unresolved container ids become
    /// JSON null so a peer id embedded in a container id cannot leave this process through the value.
    #[must_use]
    pub fn root_state_json(&self) -> Value {
        loro_value_to_json(&self.doc.get_deep_value())
    }

    /// Sets the document-level title and commits the change.
    ///
    /// # Errors
    /// Propagates the underlying engine's map-write failure, if any.
    pub fn set_title(&mut self, title: &str) -> Result<(), CollabError> {
        self.invalidate_editor_model_floor();
        self.meta
            .insert(META_TITLE, title)
            .map_err(|e| map_loro_error(&e, "<meta.title>"))?;
        self.doc.commit();
        Ok(())
    }

    /// Browser navigator ordering is an existing named map, separate from tree placement IDs.
    ///
    /// # Errors
    /// A failed root read (`editor_root_probe_failed`).
    pub fn navigator_order(&self) -> Result<BTreeMap<String, String>, CollabError> {
        let mut values = BTreeMap::new();
        let Some(order) = crate::editor_structure::root_map(&self.doc, crate::reserved_roots::ORDER)? else {
            return Ok(values);
        };
        order.for_each(|id, value| {
            if let ValueOrContainer::Value(LoroValue::String(key)) = value {
                values.insert(id.to_owned(), key.to_string());
            }
        });
        Ok(values)
    }
    /// Mutate one navigator key in place; preserve all other container identities and content.
    ///
    /// # Errors
    /// Rejects an invalid bounded key or an underlying map mutation failure.
    pub fn set_navigator_order(&mut self, id: &str, key: &str) -> Result<(), CollabError> {
        self.invalidate_editor_model_floor();
        if id.is_empty()
            || id.len() > 128
            || key.is_empty()
            || key.len() > 65
            || !key.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
        {
            return Err(CollabError::OperationFailed {
                reason: "invalid bounded navigator order key".into(),
            });
        }
        self.doc
            .get_map("order")
            .insert(id, key)
            .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
        self.doc.commit();
        Ok(())
    }

    /// Whether browser prose owns the document representation, including an empty document.
    ///
    /// # Errors
    /// A failed root read (`editor_root_probe_failed`).
    pub fn has_editor_doc(&self) -> Result<bool, CollabError> {
        editor_doc_present(&self.doc)
    }

    /// Initialize supported tree-only Page prose on a private candidate. Callers must include
    /// this delta in their normal accepted write transaction; reads never invoke this method.
    pub fn initialize_page_editor(&mut self) -> Result<bool, CollabError> {
        self.invalidate_editor_model_floor();
        if self.has_editor_doc()? {
            return Ok(false);
        }
        let legacy = self.semantic_snapshot()?;
        let mut blocks = Vec::new();
        let mut deleted = Vec::new();
        for (id, node) in &legacy.nodes {
            if node.kind != NodeKind::Block || node.properties.contains_key("embed_object_id") {
                continue;
            }
            if node.deleted {
                deleted.push(id.clone());
                continue;
            }
            if node.parent.is_some()
                || node
                    .properties
                    .keys()
                    .any(|key| !matches!(key.as_str(), "block_type" | "indent" | "level"))
            {
                return Err(CollabError::OperationFailed {
                    reason: "legacy_editor_migration_requires_flat_supported_prose".into(),
                });
            }
            let parse = |key: &str, default: u32| -> Result<u32, CollabError> {
                node.properties.get(key).map_or(Ok(default), |value| {
                    value.parse::<u32>().map_err(|_| CollabError::OperationFailed {
                        reason: "legacy_editor_migration_invalid_attribute".into(),
                    })
                })
            };
            let kind = node
                .properties
                .get("block_type")
                .cloned()
                .unwrap_or_else(|| "paragraph".into());
            let level = if kind == "heading" {
                Some(parse("level", 1)?)
            } else {
                if node.properties.contains_key("level") {
                    return Err(CollabError::OperationFailed {
                        reason: "legacy_editor_migration_invalid_level".into(),
                    });
                }
                None
            };
            blocks.push((
                node.order_key.clone(),
                id.clone(),
                crate::editor_doc_bridge::EditorInitialBlock {
                    id: id.to_string(),
                    kind,
                    text: node.text.clone(),
                    indent: parse("indent", 0)?,
                    level,
                },
            ));
        }
        if blocks.len().saturating_add(deleted.len()) > crate::limits::DOCUMENT_BLOCK_COUNT_MAX {
            return Err(CollabError::OperationFailed {
                reason: "editor_block_alias_limit_exceeded".into(),
            });
        }
        blocks.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        let blocks = blocks.into_iter().map(|(_, _, block)| block).collect::<Vec<_>>();
        let changed = crate::editor_doc_bridge::initialize_editor_doc(&self.doc, &blocks)?;
        if !changed {
            return Err(CollabError::OperationFailed {
                reason: "editor_doc_invalid_existing_children".into(),
            });
        }
        for id in deleted {
            self.doc
                .get_map("flow_editor_aliases")
                .insert(id.as_ref(), "__deleted__")
                .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })?;
        }
        self.doc.commit();
        Ok(true)
    }

    /// Includes tombstoned aliases, so a deleted editor anchor cannot be reused as tree prose.
    ///
    /// # Errors
    /// A failed root read (`editor_root_probe_failed`).
    pub fn has_editor_alias(&self, id: &str) -> Result<bool, CollabError> {
        Ok(self.has_editor_doc()? && crate::editor_doc_bridge::has_alias_entry(&self.doc, id)?)
    }

    /// Resolve only an attested live editor alias; otherwise preserve the caller's tree ID.
    ///
    /// # Errors
    /// A failed root read (`editor_root_probe_failed`).
    pub fn resolve_content_node_id(&self, id: &NodeId) -> Result<NodeId, CollabError> {
        Ok(crate::editor_doc_bridge::resolve_editor_block_id(&self.doc, id)?.map_or_else(|| id.clone(), NodeId::from))
    }

    /// Prose-command intent is explicit: typed embed planners use tree metadata blocks and
    /// must not accidentally create editor paragraphs. Text CID edits share the same path below.
    ///
    /// The result is memoized per document state (see the `editor_model_floor` field); a cached
    /// value is returned only for the exact state it was computed at.
    pub fn minimum_editor_model(&self) -> Result<u32, CollabError> {
        if let Some(cached) = self.editor_model_floor.get()
            && self.doc.get_pending_txn_len() == 0
            && cached.state == self.doc.state_frontiers()
        {
            return Ok(cached.floor);
        }
        let floor = self.compute_minimum_editor_model()?;
        if self.doc.get_pending_txn_len() == 0 {
            // A concurrent `&self` caller may have filled the cell first; it computed the same
            // value from the same unchanged state, so losing that race is harmless.
            let _ = self.editor_model_floor.set(CachedEditorModelFloor {
                state: self.doc.state_frontiers(),
                floor,
            });
        }
        Ok(floor)
    }

    /// Drops the memoized editor-model floor. Called first by every method that can change the
    /// document, before any mutation is attempted, so a failed mutation cannot leave a stale entry.
    fn invalidate_editor_model_floor(&mut self) {
        self.editor_model_floor.take();
        self.drop_session_index();
    }

    /// Drops the editor session's placement index.
    fn drop_session_index(&mut self) {
        self.session_index = None;
        self.session_stamp = None;
    }

    /// Retires the session index unless the document is exactly at the state it was sealed at.
    fn check_session_index(&mut self) {
        if self.session_index.is_some()
            && (self.doc.get_pending_txn_len() != 0 || self.session_stamp.as_ref() != Some(&self.doc.state_frontiers()))
        {
            self.drop_session_index();
        }
    }

    /// Records the document state the session index describes, after an operation committed.
    fn seal_session_index(&mut self) {
        if self.session_index.is_some() && self.doc.get_pending_txn_len() == 0 {
            self.session_stamp = Some(self.doc.state_frontiers());
        } else {
            self.drop_session_index();
        }
    }

    /// The full-document walk behind [`Self::minimum_editor_model`].
    fn compute_minimum_editor_model(&self) -> Result<u32, CollabError> {
        #[cfg(test)]
        crate::walk_counter::record_floor_walk();
        let structured = self.has_editor_structure()?;
        // Evaluated once: the block read below needs the same placements for the same state.
        let placements = if structured {
            crate::editor_structure::placements(&self.doc)?
        } else {
            None
        };
        let model5 = model5_marker(&self.doc)?;
        let physical_media = physical_atoms_present(&self.doc)?;
        let signals = crate::editor_doc_bridge::editor_model_signals(&self.doc, placements)?.unwrap_or_default();
        if model5 || physical_media || signals.atom {
            return Ok(5);
        }
        Ok(if structured || signals.beyond_model1 { 4 } else { 1 })
    }
    pub fn initialize_page_structure_model(&mut self, model: u32) -> Result<bool, CollabError> {
        if !matches!(model, 4 | 5) {
            return Err(CollabError::OperationFailed {
                reason: "editor_model_unsupported".into(),
            });
        }
        // Requested client capability is not a document minimum. Actual reference carriers
        // raise the floor; an ordinary v4 structure remains readable by a v4 client.
        self.initialize_page_structure()
    }
    /// # Errors
    /// A failed root read (`editor_root_probe_failed`).
    pub fn has_editor_structure(&self) -> Result<bool, CollabError> {
        crate::editor_structure::ready(&self.doc)
    }
    pub fn initialize_page_structure(&mut self) -> Result<bool, CollabError> {
        let floor = self.minimum_editor_model()?;
        self.invalidate_editor_model_floor();
        let changed = crate::editor_structure::initialize(&self.doc)?;
        if floor >= 5 {
            self.doc
                .get_map(crate::editor_structure::META)
                .insert("model", "rich_page_v5")
                .map_err(|error| CollabError::OperationFailed {
                    reason: error.to_string(),
                })?;
        }
        if changed {
            self.doc.commit();
        }
        Ok(changed)
    }

    pub fn insert_page_atom(
        &mut self,
        id: &str,
        parent: Option<&str>,
        index: usize,
        kind: &str,
        attrs: &Value,
    ) -> Result<String, CollabError> {
        self.invalidate_editor_model_floor();
        if !self.has_editor_doc()? {
            return Err(CollabError::OperationFailed {
                reason: "explicit_editor_initialization_required".into(),
            });
        }
        self.initialize_page_structure_model(5)?;
        let cid = crate::editor_doc_bridge::insert_atom(&self.doc, id, parent, index, kind, attrs)?;
        self.doc.commit();
        Ok(cid)
    }
    pub fn replace_page_asset(&mut self, id: &str, kind: &str, attrs: &Value) -> Result<(), CollabError> {
        self.invalidate_editor_model_floor();
        crate::editor_doc_bridge::replace_atom(&self.doc, id, kind, attrs)?;
        self.doc.commit();
        Ok(())
    }
    pub fn apply_page_marks(&mut self, id: &str, edits: &[crate::page_inline::MarkEdit]) -> Result<(), CollabError> {
        self.invalidate_editor_model_floor();
        crate::editor_doc_bridge::apply_editor_marks(&self.doc, id, edits)?;
        self.doc.commit();
        Ok(())
    }

    pub fn apply_content_operation(&mut self, operation: &Operation) -> Result<(), CollabError> {
        self.editor_model_floor.take();
        self.check_session_index();
        if let Operation::CreateNode { id, .. } = operation
            && self.id_to_tree.contains_key(id)
        {
            return Err(CollabError::DuplicateNode { id: id.to_string() });
        }
        #[cfg(test)]
        let changed = if self.recompute_placements {
            crate::editor_doc_bridge::apply_editor_structure(&self.doc, operation)
        } else {
            crate::editor_doc_bridge::apply_editor_structure_session(&self.doc, operation, &mut self.session_index)
        };
        #[cfg(not(test))]
        let changed =
            crate::editor_doc_bridge::apply_editor_structure_session(&self.doc, operation, &mut self.session_index);
        if changed.inspect_err(|_| self.drop_session_index())? {
            self.doc.commit();
            self.seal_session_index();
            return Ok(());
        }
        self.apply_operation(operation)
    }

    pub fn apply_operation(&mut self, operation: &Operation) -> Result<(), CollabError> {
        self.editor_model_floor.take();
        self.check_session_index();
        let result = self.apply_operation_unchecked(operation);
        match result {
            Ok(()) => self.seal_session_index(),
            Err(_) => self.drop_session_index(),
        }
        result
    }

    /// One text edit of an editor block, located through the session index.
    fn apply_editor_text_edit(
        &mut self,
        id: &str,
        index: u32,
        delete_len: Option<u32>,
        insert: Option<&str>,
    ) -> Result<bool, CollabError> {
        #[cfg(test)]
        if self.recompute_placements {
            return crate::editor_doc_bridge::apply_editor_text(&self.doc, id, index, delete_len, insert);
        }
        crate::editor_doc_bridge::apply_editor_text_session(
            &self.doc,
            id,
            index,
            delete_len,
            insert,
            &mut self.session_index,
        )
    }

    fn apply_operation_unchecked(&mut self, operation: &Operation) -> Result<(), CollabError> {
        let editor_changed = match operation {
            Operation::InsertText { id, index, text } => self.apply_editor_text_edit(id, *index, None, Some(text))?,
            Operation::DeleteText { id, index, len } => self.apply_editor_text_edit(id, *index, Some(*len), None)?,
            _ => false,
        };
        if editor_changed {
            self.doc.commit();
            return Ok(());
        }
        // Browser prose has a separate canonical Loro container. A legacy tree-text write
        // cannot be acknowledged if it would only change the shadow tree that readers suppress.
        // The explicit editor-block bridge handles its own container identities separately.
        if matches!(operation, Operation::InsertText { .. } | Operation::DeleteText { .. }) && self.has_editor_doc()? {
            return Err(CollabError::OperationFailed {
                reason: "editor_doc_requires_editor_block_command".into(),
            });
        }
        match operation {
            Operation::CreateNode {
                id,
                parent,
                index,
                kind,
            } => {
                if self.id_to_tree.contains_key(id)
                    || (self.has_editor_doc()?
                        && (crate::editor_doc_bridge::has_alias_entry(&self.doc, id.as_ref())?
                            || crate::editor_doc_bridge::resolve_editor_block_id(&self.doc, id)?.is_some()))
                {
                    return Err(CollabError::DuplicateNode { id: id.to_string() });
                }
                let parent_id = self.resolve_parent(parent.as_ref())?;
                let safe_index = self.clamp_index(parent_id, *index);
                let tree_id = self
                    .tree
                    .create_at(parent_id, safe_index)
                    .map_err(|e| map_loro_error(&e, id))?;
                let meta = self.tree.get_meta(tree_id).map_err(|e| map_loro_error(&e, id))?;
                meta.insert(META_LOGICAL_ID, id.as_ref())
                    .map_err(|e| map_loro_error(&e, id))?;
                meta.insert(META_KIND, kind_to_str(*kind))
                    .map_err(|e| map_loro_error(&e, id))?;
                self.doc.commit();
                self.id_to_tree.insert(id.clone(), tree_id);
                self.tree_to_id.insert(tree_id, id.clone());
                Ok(())
            }
            Operation::MoveNode { id, new_parent, index } => {
                let tree_id = self.require_tree_id(id)?;
                let parent_id = self.resolve_parent(new_parent.as_ref())?;
                let safe_index = self.clamp_index(parent_id, *index);
                self.tree
                    .mov_to(tree_id, parent_id, safe_index)
                    .map_err(|e| map_loro_error(&e, id))?;
                self.doc.commit();
                Ok(())
            }
            Operation::DeleteNode { id } => {
                let tree_id = self.require_tree_id(id)?;
                self.tree.delete(tree_id).map_err(|e| map_loro_error(&e, id))?;
                self.doc.commit();
                Ok(())
            }
            Operation::InsertText { id, index, text } => {
                let tree_id = self.require_tree_id(id)?;
                let handler = self.text_handler(tree_id)?;
                handler
                    .insert_utf8(*index as usize, text)
                    .map_err(|e| map_loro_error(&e, id))?;
                self.doc.commit();
                Ok(())
            }
            Operation::DeleteText { id, index, len } => {
                let tree_id = self.require_tree_id(id)?;
                let handler = self.text_handler(tree_id)?;
                handler
                    .delete_utf8(*index as usize, *len as usize)
                    .map_err(|e| map_loro_error(&e, id))?;
                self.doc.commit();
                Ok(())
            }
            Operation::SetProperty { id, key, value } => {
                let tree_id = self.require_tree_id(id)?;
                let meta = self.tree.get_meta(tree_id).map_err(|e| map_loro_error(&e, id))?;
                let full_key = format!("{META_PROPERTY_PREFIX}{key}");
                meta.insert(&full_key, value.as_str())
                    .map_err(|e| map_loro_error(&e, id))?;
                self.doc.commit();
                Ok(())
            }
        }
    }

    /// Exports the current merged tree state as the engine-independent [`SemanticSnapshot`].
    /// Document-level fields (`title`) are not part of this shape; read them via [`Self::title`].
    ///
    /// # Errors
    /// Propagates the underlying engine's read failure, if any.
    pub fn semantic_snapshot(&self) -> Result<SemanticSnapshot, CollabError> {
        self.semantic_snapshot_with_floor().map(|(snapshot, _)| snapshot)
    }

    /// [`Self::semantic_snapshot`] together with [`Self::minimum_editor_model`], derived from the
    /// block pass the projection already makes instead of a second walk.
    ///
    /// The floor equals `compute_minimum_editor_model()` whenever both succeed. The converse does
    /// not hold: the projection reads more than the walk does, so it can fail on a document the
    /// walk accepts (for example an unplaced block map without `nodeName` on a structured page).
    /// Nothing is memoized in that case, and callers treat a failed projection as a refusal. On
    /// success the memo is filled under the same
    /// rules as `minimum_editor_model` (no pending local transaction, keyed to the current state
    /// frontier), so later readers of the same state cost no walk. Nothing is memoized on error.
    ///
    /// # Errors
    /// Propagates the underlying engine's read failure, if any.
    pub fn semantic_snapshot_with_floor(&self) -> Result<(SemanticSnapshot, u32), CollabError> {
        // `legacy_package_semantic_snapshot_v3` followed by `project_editor_prose`, reading the
        // editor document once for both prose steps (see
        // `project_legacy_then_current_editor_prose`).
        let mut snapshot = self.legacy_package_semantic_snapshot_v1()?;
        self.project_navigator_order(&mut snapshot)?;
        let facts = crate::prose_projection::project_legacy_then_current_editor_prose(&self.doc, &mut snapshot)?;
        let floor = if model5_marker(&self.doc)? || facts.physical_atom || facts.signals.atom {
            5
        } else if self.has_editor_structure()? || facts.signals.beyond_model1 {
            4
        } else {
            1
        };
        if self.doc.get_pending_txn_len() == 0 {
            // Losing a fill race is harmless: the other writer computed the same value.
            let _ = self.editor_model_floor.set(CachedEditorModelFloor {
                state: self.doc.state_frontiers(),
                floor,
            });
        }
        Ok((snapshot, floor))
    }

    /// The Loro document, for the incremental window check.
    pub(crate) const fn doc(&self) -> &LoroDoc {
        &self.doc
    }

    /// Whether some logical id is claimed by two tree nodes (see `duplicate_logical_ids`).
    /// Whether the legacy tree holds `id`, live or deleted.
    pub(crate) fn has_tree_node(&self, id: TreeID) -> bool {
        self.tree.contains(id)
    }

    pub(crate) const fn has_duplicate_logical_ids(&self) -> bool {
        self.duplicate_logical_ids
    }

    /// Whether a tree node has this logical id.
    pub(crate) fn has_logical_id(&self, id: &str) -> bool {
        self.id_to_tree.contains_key(id)
    }

    /// The block maps in `doc.children`, in list order, as container ids: `None` without an editor
    /// document. Read from the list's value, which names every element by its container id and
    /// costs far less than reading the blocks.
    ///
    /// # Errors
    /// A failed root read, or an element that is not a map container.
    pub(crate) fn root_block_ids(&self) -> Result<Option<Vec<loro::ContainerID>>, CollabError> {
        let Some(root) = crate::editor_structure::root_map(&self.doc, crate::reserved_roots::DOC)? else {
            return Ok(None);
        };
        let Some(ValueOrContainer::Container(Container::List(list))) = root.get("children") else {
            return Ok(None);
        };
        let LoroValue::List(items) = list.get_value() else {
            return Err(CollabError::OperationFailed {
                reason: "editor_doc_unsupported_block_container".into(),
            });
        };
        items
            .iter()
            .map(|item| match item {
                LoroValue::Container(id @ loro::ContainerID::Normal { container_type, .. })
                    if *container_type == loro::ContainerType::Map =>
                {
                    Ok(id.clone())
                }
                _ => Err(CollabError::OperationFailed {
                    reason: "editor_doc_unsupported_block_container".into(),
                }),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    /// The items (see [`crate::work`]) of everything the full projection reads: the tree nodes
    /// with their metadata, the elements of the root block list with everything below them, and
    /// the entries, keys and value payload of the navigator `order` map and of the alias map. `Ok(None)` once the count
    /// passes `ceiling`, so a page far over any budget costs no more than the ceiling to refuse.
    ///
    /// # Errors
    /// A failed read of a node's metadata, which fails the projection as well.
    #[cfg(test)]
    pub(crate) fn projection_items(&self, ceiling: u64) -> Result<Option<u64>, CollabError> {
        Ok(match self.count_projection(crate::work::ItemCount::new(ceiling))? {
            ProjectionCount::Within { items, .. } => Some(items),
            ProjectionCount::OverCeiling | ProjectionCount::GuardFired(_) => None,
        })
    }

    /// [`Self::projection_items`] for the setup: it also yields the mark items of every block, and
    /// stops with [`ProjectionCount::GuardFired`] once `guard` has passed a limit.
    pub(crate) fn projection_count_guarded(
        &self,
        ceiling: u64,
        guard: crate::setup_guard::SetupGuard,
    ) -> Result<ProjectionCount, CollabError> {
        self.count_projection(crate::work::ItemCount::guarded(ceiling, guard))
    }

    fn count_projection(&self, mut items: crate::work::ItemCount) -> Result<ProjectionCount, CollabError> {
        use crate::reserved_roots::{ALIASES, DOC, ORDER};
        let stopped = |items: &crate::work::ItemCount| {
            items
                .stopped_by_guard()
                .map_or(ProjectionCount::OverCeiling, ProjectionCount::GuardFired)
        };
        for name in [ORDER, ALIASES] {
            // Entries, keys and the payload of the values: the full projection clones all of them
            // (`navigator_order`), and a client update writes the map without the server's bounds.
            if let Some(map) = crate::editor_structure::root_map(&self.doc, name)?
                && !items.add_container(&Container::Map(map), 0)
            {
                return Ok(stopped(&items));
            }
        }
        if let Some(root) = crate::editor_structure::root_map(&self.doc, DOC)?
            && let Some(ValueOrContainer::Container(Container::List(list))) = root.get("children")
        {
            if !items.add(u64::try_from(list.len()).unwrap_or(u64::MAX)) {
                return Ok(stopped(&items));
            }
            let mut within = true;
            list.for_each(|element| {
                if within && let ValueOrContainer::Container(container) = element {
                    within = items.add_container(&container, 0)
                        && match &container {
                            Container::Map(block) => items.add_block_marks(block),
                            _ => true,
                        };
                }
            });
            if !within {
                return Ok(stopped(&items));
            }
        }
        // The projection reads a legacy node's text as a plain string, so its marks cost nothing.
        for tree_id in self.tree.nodes() {
            let meta = self.tree.get_meta(tree_id).map_err(|e| map_loro_error(&e, "<work>"))?;
            if !items.add(1) || !items.add_container(&Container::Map(meta), 0) {
                return Ok(stopped(&items));
            }
        }
        let total = items.total();
        Ok(ProjectionCount::Within {
            items: total,
            marks: items.into_marks(),
        })
    }

    /// The mark items (see [`crate::work::block_mark_items`]) of every editor block that has any, by
    /// the block's key. Only the inline texts the block reader converts count: the projection reads
    /// no other text's marks (a legacy node's text is a plain string), so nothing else is converted
    /// here, in setup or anywhere. The setup does not call it: the count of the page's items already
    /// holds these figures (`ProjectionCount::Within`), and a second pass would convert every
    /// marked text once more. It stays as the reference the tests compare that count with.
    #[cfg(test)]
    pub(crate) fn unit_mark_items(&self) -> Option<HashMap<NodeId, u64>> {
        let mut found = HashMap::new();
        for id in self.root_block_ids().ok()?.unwrap_or_default() {
            if let Some(Container::Map(block)) = self.doc.get_container(id.clone()) {
                let items = crate::work::block_mark_items(&block, u64::MAX)?;
                if items > 0 {
                    found.insert(NodeId::from(id.to_string()), items);
                }
            }
        }
        Some(found)
    }

    /// What the touched blocks' mark items grew by since `facts`, summed over the blocks that grew
    /// (a block that shrank counts nothing, so the sum stays an upper bound).
    ///
    /// The walk of a block stops its value payload as soon as the block's items pass its base figure
    /// plus the `headroom` that is left; the conversion of a text's delta itself cannot stop early,
    /// and is bounded by the base figure (which the facts counted) and by the metered import (which
    /// paid for every attribute the update added). `marked` says that the update carries a mark op.
    /// `None` means the budget cannot hold the result, and the caller falls back.
    pub(crate) fn mark_items_added(
        &self,
        facts: &crate::facts::BaseFacts,
        blocks: &HashSet<loro::ContainerID>,
        marked: bool,
        headroom: u64,
    ) -> Option<u64> {
        let mut added: u64 = 0;
        for id in blocks {
            let Some(Container::Map(block)) = self.doc.get_container(id.clone()) else {
                continue;
            };
            let key = id.to_string();
            let before = facts.marks.get(key.as_str()).copied().unwrap_or(0);
            // A block that the projection read with no mark items and whose update carries no mark op
            // has none now: with `before == 0` every text is one segment with at most two attributes
            // the reader skips, an insert op adds at most two pieces (108 bytes, 1.7 items) and is
            // charged two items at least (`work::op_work`), so its pieces cannot reach an item.
            // The constants that this depends on are pinned by
            // `work::tests::the_pieces_an_insert_can_add_to_an_unmarked_text_cost_less_than_the_insert_is_charged`. A block the projection did not read (no
            // unit) may have become readable through this update, and is counted.
            if before == 0 && !marked && facts.units.contains_key(key.as_str()) {
                continue;
            }
            let left = headroom.saturating_sub(added);
            let after = crate::work::block_mark_items(&block, before.saturating_add(left))?;
            added = added.saturating_add(after.saturating_sub(before));
        }
        Some(added)
    }

    /// The number of elements of the root block list, of any kind: `None` without an editor
    /// document. O(1), and it reads no element.
    #[cfg(test)]
    pub(crate) fn root_list_len(&self) -> Option<usize> {
        let root = crate::editor_structure::root_map(&self.doc, crate::reserved_roots::DOC)
            .ok()
            .flatten()?;
        match root.get("children") {
            Some(ValueOrContainer::Container(Container::List(list))) => Some(list.len()),
            _ => None,
        }
    }

    /// Whether the nodes in `created` have no children but nodes in `created`: every child that a
    /// created node has now is itself a created node. A node that is not in the tree (its parent
    /// was missing) has no children to speak of.
    ///
    /// Counts instead of listing: the created nodes whose parent is a created node are tallied
    /// per parent, and each created node must have exactly that many children.
    pub(crate) fn created_nodes_adopt_nothing(&self, created: &HashSet<TreeID>) -> bool {
        let mut created_children: HashMap<TreeID, usize> = HashMap::new();
        for node in created {
            if !self.tree.contains(*node) {
                continue;
            }
            if let Some(TreeParentId::Node(parent)) = self.tree.parent(*node)
                && created.contains(&parent)
            {
                *created_children.entry(parent).or_default() += 1;
            }
        }
        created.iter().all(|node| {
            !self.tree.contains(*node)
                // The library answers `None` for a node that has no children at all.
                || self.tree.children_num(TreeParentId::Node(*node)).unwrap_or(0)
                    == created_children.get(node).copied().unwrap_or(0)
        })
    }

    /// What [`Self::semantic_snapshot`] makes of one legacy tree node, reduced to what the
    /// structural limits read: its key, its parent's key, whether the projection keeps it, whether
    /// it is deleted, its kind and the characters it contributes. `editor_doc` is whether the page
    /// has an editor document, which drops nodes marked as editor blocks and clears the text of
    /// block nodes (`prose_projection::project_blocks`). `None` for a node without a logical id,
    /// which the projection skips.
    ///
    /// It reads the same fields in the same way as `legacy_package_semantic_snapshot_v1` and
    /// `project_blocks`.
    ///
    /// # Errors
    /// The failed read of the node's deleted state or metadata, which fails the whole projection.
    pub(crate) fn evaluate_tree_unit(
        &self,
        tree_id: TreeID,
        editor_doc: bool,
    ) -> Result<Option<TreeUnit>, CollabError> {
        let Some(key) = self.tree_to_id.get(&tree_id).cloned() else {
            return Ok(None);
        };
        let deleted = self
            .tree
            .is_node_deleted(&tree_id)
            .map_err(|e| map_loro_error(&e, &key))?;
        let parent_tp = self.tree.parent(tree_id).unwrap_or(TreeParentId::Root);
        let parent_key = match parent_tp {
            TreeParentId::Node(parent_tree_id) => self.tree_to_id.get(&parent_tree_id).cloned(),
            _ => None,
        };
        let meta = self.tree.get_meta(tree_id).map_err(|e| map_loro_error(&e, &key))?;
        let kind = meta
            .get(META_KIND)
            .and_then(|v| read_string_value(&v))
            .map_or(NodeKind::Block, |s| str_to_kind(&s));
        let property = |name: &str| {
            meta.get(&format!("{META_PROPERTY_PREFIX}{name}"))
                .and_then(|raw| read_string_value(&raw))
        };
        let present = !(editor_doc && property("content_source").as_deref() == Some("editor_doc"));
        let cleared = editor_doc && kind == NodeKind::Block && property("embed_object_id").is_none();
        let chars = if !present || deleted || cleared {
            0
        } else {
            match meta.get(META_TEXT) {
                Some(ValueOrContainer::Container(Container::Text(text))) => text.to_string().chars().count(),
                _ => 0,
            }
        };
        Ok(Some(TreeUnit {
            key,
            parent_key,
            present,
            deleted,
            kind,
            chars,
        }))
    }

    /// Frozen v4 prose/Tree reader. A model5 document must declare the v5 package model.
    pub fn legacy_package_semantic_snapshot_v4(&self) -> Result<SemanticSnapshot, CollabError> {
        if self.minimum_editor_model()? > 4 {
            return Err(CollabError::OperationFailed {
                reason: "v4_reader_requires_model4_document".into(),
            });
        }
        let mut snapshot = self.legacy_package_semantic_snapshot_v3()?;
        crate::prose_projection::project_editor_prose(&self.doc, &mut snapshot)?;
        Ok(snapshot)
    }
    pub fn legacy_package_semantic_snapshot_v3(&self) -> Result<SemanticSnapshot, CollabError> {
        let mut snapshot = self.legacy_package_semantic_snapshot_v1()?;
        self.project_navigator_order(&mut snapshot)?;
        crate::prose_projection::project_legacy_editor_prose(&self.doc, &mut snapshot)?;
        Ok(snapshot)
    }

    /// Package v3's navigator step: order keys from the `order` map override tree positions.
    fn project_navigator_order(&self, snapshot: &mut SemanticSnapshot) -> Result<(), CollabError> {
        let order = self.navigator_order()?;
        for (id, node) in &mut snapshot.nodes {
            if node.kind != NodeKind::NavigatorNode {
                continue;
            }
            let direct = order.get(id.as_ref());
            let generated = id.split_once('#').and_then(|(base, generation)| {
                let valid = base.len() == 36
                    && base.bytes().enumerate().all(|(i, b)| {
                        if [8, 13, 18, 23].contains(&i) {
                            b == b'-'
                        } else {
                            b.is_ascii_hexdigit()
                        }
                    });
                (valid && !generation.is_empty() && generation.bytes().all(|b| b.is_ascii_digit()))
                    .then(|| order.get(base))
                    .flatten()
            });
            if let Some(key) = direct.or(generated) {
                node.order_key.clone_from(key);
            }
        }
        Ok(())
    }

    /// Exact package v2 reader before navigator order-map projection (tree plus editor prose).
    ///
    /// # Errors
    /// Propagates tree or editor-prose decoding errors without changing the document.
    pub fn legacy_package_semantic_snapshot_v2(&self) -> Result<SemanticSnapshot, CollabError> {
        let mut snapshot = self.legacy_package_semantic_snapshot_v1()?;
        crate::prose_projection::project_legacy_editor_prose(&self.doc, &mut snapshot)?;
        Ok(snapshot)
    }

    /// Exact tree-only semantic reader used by package v1 before editor prose projection.
    /// Restricted to versioned package fingerprints; current reads must use `semantic_snapshot`.
    pub fn legacy_package_semantic_snapshot_v1(&self) -> Result<SemanticSnapshot, CollabError> {
        let mut snapshot = SemanticSnapshot::default();
        // Every node's `order_key` needs its position among its parent's children. The
        // straightforward per-node `siblings.iter().position(...)` (this method's shape before
        // this fix) is O(sibling count) *per node*, so a parent with many children makes building
        // the whole snapshot O(n^2) in that group's size -- this package's isolated-apply
        // delivery report has the measured before/after (a document approaching
        // `document_block_count_max`/`container_count_max` was taking longer than
        // `decode_apply_cpu_ms_max` to snapshot on its own, well before reaching the ceiling that
        // is supposed to bound it). Memoizing each distinct parent's children into a `TreeID ->
        // position` index the first time it is needed, and reusing that index for every other
        // node under the same parent, makes this O(n) overall instead: `self.tree.children` is
        // still called once per distinct parent (not once per node), and each lookup afterwards
        // is O(1).
        let mut position_cache: HashMap<TreeParentId, HashMap<TreeID, usize>> = HashMap::new();
        for tree_id in self.tree.nodes() {
            let Some(logical_id) = self.tree_to_id.get(&tree_id).cloned() else {
                // A node this replica has never resolved to a logical id (shouldn't happen once
                // `rebuild_id_cache` has run, but skip defensively rather than panic).
                continue;
            };
            let deleted = self
                .tree
                .is_node_deleted(&tree_id)
                .map_err(|e| map_loro_error(&e, &logical_id))?;
            let parent_tp = self.tree.parent(tree_id).unwrap_or(TreeParentId::Root);
            let parent_logical = match parent_tp {
                TreeParentId::Node(parent_tree_id) => self.tree_to_id.get(&parent_tree_id).cloned(),
                _ => None,
            };
            let positions = position_cache.entry(parent_tp).or_insert_with(|| {
                self.tree
                    .children(parent_tp)
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                    .map(|(index, id)| (id, index))
                    .collect()
            });
            let order_key = zero_padded_order_key(positions.get(&tree_id).copied().unwrap_or(0));

            let meta = self
                .tree
                .get_meta(tree_id)
                .map_err(|e| map_loro_error(&e, &logical_id))?;
            let kind = meta
                .get(META_KIND)
                .and_then(|v| read_string_value(&v))
                .map_or(NodeKind::Block, |s| str_to_kind(&s));
            let text = match meta.get(META_TEXT) {
                Some(ValueOrContainer::Container(Container::Text(text_container))) => text_container.to_string(),
                _ => String::new(),
            };
            let mut properties = BTreeMap::new();
            for key in meta.keys() {
                let key_str: &str = key.as_ref();
                if let Some(prop_name) = key_str.strip_prefix(META_PROPERTY_PREFIX)
                    && let Some(raw) = meta.get(key_str)
                    && let Some(value) = read_string_value(&raw)
                {
                    properties.insert(prop_name.to_string(), value);
                }
            }

            snapshot.nodes.insert(
                logical_id,
                SemanticNode {
                    parent: parent_logical,
                    order_key,
                    kind,
                    text,
                    properties,
                    deleted,
                },
            );
        }
        Ok(snapshot)
    }
}

/// One legacy tree node as the projection sees it, reduced to what the structural limits read.
/// See [`LoroCollabEngine::evaluate_tree_unit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TreeUnit {
    pub key: NodeId,
    pub parent_key: Option<NodeId>,
    /// The projection keeps the node.
    pub present: bool,
    pub deleted: bool,
    pub kind: NodeKind,
    /// Characters the node contributes when it is live (zero when its text is cleared).
    pub chars: usize,
}

/// What an import left behind for the incremental window check.
pub(crate) struct ImportTrace {
    /// Whether the document's state changed.
    pub changed: bool,
    /// The state version vector before the import.
    pub before_state: VersionVector,
    /// The library's account of the import: the imported spans, nothing pending (a causal import
    /// refuses otherwise).
    pub status: loro::ImportStatus,
    /// The units the imported ops touched (client imports only; empty for replay).
    pub summary: crate::window_check::OpSummary,
    /// An existing tree node got a `logical_id` by this import, or the id caches were rebuilt in
    /// full (so the set of nodes with an id may differ from the base's anywhere).
    pub resolved_existing_id: bool,
    /// The import was a strict causal extension of the base (`OpSummary::strictly_sequential`).
    pub sequential: bool,
}

/// How an import is judged: replay of accepted history, or an update a client offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportMode {
    /// Accepted history. `causal` refuses an update with missing dependencies.
    Replay { causal: bool },
    /// A client update: causal, and no new shadow roots.
    Client,
}

impl LoroCollabEngine {
    /// [`CollabEngine::import_update`] for an update offered for acceptance: additionally refuses
    /// an update whose causal dependencies are not in this document with
    /// [`CollabError::CausalDependencyMissing`], instead of reporting it as an applied (empty)
    /// change.
    ///
    /// The plain `import_update` stays lenient on purpose: it replays history that was already
    /// accepted, where refusing now would make a stored document unreadable.
    ///
    /// # Errors
    /// Everything `import_update` reports, plus `CausalDependencyMissing`. After that error the
    /// engine still buffers the pending changes internally (the visible state and frontier are
    /// unchanged), so the caller must discard it -- as the isolated-apply worker, whose engine
    /// lives for one request, does.
    pub fn import_update_causally_complete(&mut self, update: &[u8]) -> Result<Diff, CollabError> {
        self.import_guarded(update, ImportMode::Replay { causal: true })
    }

    /// [`Self::import_update_causally_complete`] for an update that a client offers for
    /// acceptance: additionally refuses an update that creates or writes a reserved root container
    /// (`reserved_roots`) with a type other than the expected one, a "shadow root", with
    /// `reserved_root_container_type`, and an update whose ops cannot all be inspected with
    /// `reserved_root_scan_incomplete`.
    ///
    /// Only this entry applies the rule. Replay of history that was accepted earlier
    /// (`import_update`, `import_update_causally_complete`) must keep working for a document that
    /// already contains a shadow root. See `floor_guard::scan_imported_ops` for what the rule rests
    /// on. The cost is O(imported ops) and shares the pass that the floor guard makes anyway.
    ///
    /// # Errors
    /// Everything `import_update_causally_complete` reports, plus the two refusals above. After
    /// any error the engine must be discarded: the update is already applied to it, as for the
    /// floor guard's refusal.
    pub fn import_client_update(&mut self, update: &[u8]) -> Result<Diff, CollabError> {
        self.import_guarded(update, ImportMode::Client)
    }

    /// Refuses a document that carries a reserved root container of the wrong type (a "shadow
    /// root"), in its state or anywhere in its history. For a document that arrives from outside,
    /// such as an uploaded package: O(history), so never for the write window. Replay of the
    /// server's own stored history does not call it. See [`Self::import_client_update`].
    ///
    /// # Errors
    /// `reserved_root_container_type`, or `reserved_root_scan_incomplete` when the history cannot
    /// be inspected (a shallow document).
    pub fn refuse_shadow_roots(&self) -> Result<(), CollabError> {
        crate::floor_guard::refuse_shadow_roots(&self.doc)
    }

    /// The host's check of the document a worker returned for an update on `base_snapshot`: the ops
    /// it gained over the base are read with the rules of [`crate::tree_refs`]. The base's version
    /// is read from the snapshot header, so the base is not loaded.
    ///
    /// # Errors
    /// `tree_parent_missing`, `tree_parent_cycle`, or a header that cannot be read.
    pub fn refuse_unresolved_tree_parents_over(&self, base_snapshot: &[u8]) -> Result<(), CollabError> {
        crate::tree_refs::refuse_unresolved_tree_parents_over(&self.doc, base_snapshot)
    }

    /// Refuses a document whose history holds a tree op that names a parent that does not exist
    /// when the op runs ([`crate::tree_refs`]), for a document that arrives from outside, such as
    /// an uploaded package: O(history), so never for the write window.
    ///
    /// # Errors
    /// `tree_parent_missing`, `tree_parent_cycle`, or `reserved_root_scan_incomplete` for a
    /// document whose history cannot be read.
    pub fn refuse_unresolved_tree_parents_in_history(&self) -> Result<(), CollabError> {
        if self.doc.is_shallow() {
            return Err(CollabError::OperationFailed {
                reason: crate::reserved_roots::SCAN_INCOMPLETE_REASON.into(),
            });
        }
        crate::tree_refs::refuse_unresolved_tree_parents(&self.doc, &loro::VersionVector::default())
    }

    /// Materialises, ahead of a write, the parts of the *trusted base* that the import of a client
    /// update would otherwise build inside the metered window: the root key index (the first root
    /// lookup lists the keys of a lazily loaded store), and the decoded state of the containers a
    /// block insert touches, the root block list (`doc.children`), the alias map and the structure
    /// placement tree, and the navigator `order` map. The `order` map is the one container beyond
    /// the first three that the approved list (root key index, root block list, alias map, placement
    /// tree) does not name: it is the same kind of work, decoding trusted base container state
    /// before the timer, a pure function of the base that depends on no update and reaches no
    /// verdict, and it is decoded here because the projection reads it on every window.
    ///
    /// Loro decodes the full state of every container an import changes, so without this the
    /// first import of a block insert costs O(page); the reads below are the public ones that
    /// force that decode (`LoroList::get_id_at`, `LoroMap::get_last_editor`, `LoroTree::roots`).
    /// Handler reads such as `get`, `len` and `get_value` do not, which is why a pin test checks
    /// the effect on the allocation of the next import instead of trusting the call.
    ///
    /// It is a pure function of the document it is called on: it reads only reserved roots that
    /// exist (a read registers nothing, see `editor_structure::root_container`), writes nothing,
    /// reaches no verdict and never sees an update. It must run before the update is known to the
    /// engine, outside the metered window. A failed root read is not reported: it only means that
    /// less was materialised, and the window repeats the same read and refuses by itself.
    pub fn prewarm_for_writes(&self) {
        self.prewarm_parts(PrewarmParts::ALL);
    }

    /// [`Self::prewarm_for_writes`] for a chosen set of containers. The root key index is always
    /// built, by the first root lookup.
    pub(crate) fn prewarm_parts(&self, parts: PrewarmParts) {
        use crate::reserved_roots::{ALIASES, DOC, ORDER, TREE};
        // The first lookup builds the root index; the later ones reuse it.
        let root = crate::editor_structure::root_map(&self.doc, DOC);
        if parts.has(PrewarmParts::BLOCK_LIST)
            && let Ok(Some(root)) = root
            && let Some(ValueOrContainer::Container(Container::List(list))) = root.get("children")
        {
            let _ = list.get_id_at(0);
        }
        if parts.has(PrewarmParts::ALIASES)
            && let Ok(Some(aliases)) = crate::editor_structure::root_map(&self.doc, ALIASES)
        {
            let _ = aliases.get_last_editor("");
        }
        if parts.has(PrewarmParts::ORDER)
            && let Ok(Some(order)) = crate::editor_structure::root_map(&self.doc, ORDER)
        {
            let _ = order.get_last_editor("");
        }
        if parts.has(PrewarmParts::STRUCTURE_TREE)
            && let Ok(Some(Container::Tree(tree))) = crate::editor_structure::reserved_root(&self.doc, TREE)
        {
            let _ = tree.roots();
        }
    }

    /// Imports `update` and refuses an update that takes the editor-model floor from 5 to below 5
    /// (`editor_model_floor_downgrade`), without walking the document in the common case.
    ///
    /// The decision equals "floor before >= 5 and floor after < 5" (`compute_minimum_editor_model`
    /// on both states) for every update, provided the base document is valid. Floor >= 5 is the
    /// model-5 marker or a physical atom block map; the marker is read before and after in O(1)
    /// and the imported ops are scanned in O(update) (see `floor_guard`). Only an update that
    /// deletes from the top-level block list (whatever ids it names), re-types a pre-existing block
    /// map, or touches the `doc` map falls back to a
    /// narrow scan of `doc.children` and, if still undecided, a read of the pre-import state.
    fn import_guarded(&mut self, update: &[u8], mode: ImportMode) -> Result<Diff, CollabError> {
        self.import_guarded_traced(update, mode, None)
            .map(|trace| Diff { changed: trace.changed })
    }

    /// [`Self::import_client_update`], keeping what the incremental window check needs to know
    /// about the import: the state before it, the library's [`ImportStatus`] and the units the
    /// update's ops touched. The decisions and the effect on the document are the same.
    ///
    /// # Errors
    /// As [`Self::import_client_update`].
    pub(crate) fn import_client_update_traced(
        &mut self,
        update: &[u8],
        base: Option<&crate::facts::BaseVersion>,
    ) -> Result<ImportTrace, CollabError> {
        self.import_guarded_traced(update, ImportMode::Client, base)
    }

    fn import_guarded_traced(
        &mut self,
        update: &[u8],
        mode: ImportMode,
        base: Option<&crate::facts::BaseVersion>,
    ) -> Result<ImportTrace, CollabError> {
        let causal = !matches!(mode, ImportMode::Replay { causal: false });
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
        if causal && status.pending.is_some() {
            return Err(CollabError::CausalDependencyMissing);
        }
        let after = self.doc.state_vv();
        let mut summary = crate::window_check::OpSummary::default();
        let client_scan = if matches!(mode, ImportMode::Client) {
            Some(crate::floor_guard::scan_imported_ops_visiting(
                &self.doc,
                &before_ops,
                true,
                &mut |scanned| match scanned {
                    crate::floor_guard::Scanned::Change(change) => summary.visit_change(change),
                    crate::floor_guard::Scanned::Op(peer, op, root_list) => summary.visit(peer, op, root_list),
                },
            )?)
        } else {
            None
        };
        // A tree op that names a parent which does not exist when it runs would panic every
        // subscribed client that imports the update: refused here, in the window, on every path.
        if client_scan.is_some() {
            summary.refuse_unresolved_tree_parents(&self.doc, &before_ops)?;
        }
        // The id caches are rebuilt from the update's own ops only for a strictly sequential
        // import (`OpSummary::strictly_sequential`), the only one that cannot add a node no op of
        // it names, and only when the ops name every tree node they created (no move or delete,
        // no create it cannot match to its own id). Everything else rebuilds in full, exactly as
        // `next` does (`rebuild_id_cache`), and the window then takes the full computation.
        let sequential = client_scan.is_some()
            && base.is_some_and(|base| {
                injected_fault_sequential_gate_removed() || summary.strictly_sequential(base, &before_ops)
            });
        let incremental_cache = sequential && summary.tree_ids_known();
        let resolved_existing_id = if incremental_cache {
            self.rebuild_id_cache_for(&summary.tree_created)?
        } else {
            self.rebuild_id_cache()?;
            true
        };
        self.id_cache_incremental = incremental_cache;
        self.enforce_floor_ratchet(prior_m5, &before_ops, &before_frontiers, client_scan)?;
        Ok(ImportTrace {
            changed: before != after,
            before_state: before,
            status,
            summary,
            resolved_existing_id,
            sequential,
        })
    }

    /// The downgrade guard: `Err(editor_model_floor_downgrade)` exactly when the floor was >= 5
    /// before the import (`prior_m5` or a physical atom at `before`) and is below 5 after it.
    fn enforce_floor_ratchet(
        &self,
        prior_m5: bool,
        before_ops: &VersionVector,
        before: &Frontiers,
        scanned: Option<crate::floor_guard::RemovalScan>,
    ) -> Result<(), CollabError> {
        use crate::floor_guard::RemovalScan;
        let downgrade = || CollabError::OperationFailed {
            reason: "editor_model_floor_downgrade".into(),
        };
        if model5_marker(&self.doc)? {
            return Ok(());
        }
        let narrow_atom_scan = || {
            #[cfg(test)]
            crate::walk_counter::record_narrow_atom_scan();
            physical_atoms_present(&self.doc)
        };
        let removal = match scanned {
            Some(removal) => removal,
            None => crate::floor_guard::removal_scan(&self.doc, before_ops)?,
        };
        match (prior_m5, removal) {
            // No block map that could hold the physical-atom signal up was removed or re-typed,
            // and the marker was not set before: if the signal was set before it still is.
            (false, RemovalScan::None) => Ok(()),
            // The marker held the floor at 5 and is gone: only a physical atom keeps it there.
            (true, _) => {
                if narrow_atom_scan()? {
                    Ok(())
                } else {
                    Err(downgrade())
                }
            }
            (false, RemovalScan::Suspect) => {
                // Before the import the floor was >= 5 only through a physical atom. If an atom
                // is still present the floor is still 5. Otherwise the base decides, exactly. A
                // result that cannot be read as floor 5 is a refusal only if the base was 5.
                let after = narrow_atom_scan();
                if matches!(after, Ok(true)) || !crate::floor_guard::physical_atoms_at(&self.doc, before)? {
                    return Ok(());
                }
                after.and_then(|_| Err(downgrade()))
            }
        }
    }
}

impl CollabEngine for LoroCollabEngine {
    type Error = CollabError;

    fn load(snapshot: &[u8]) -> Result<Self, Self::Error> {
        InputLimits::default().validate_snapshot(snapshot)?;
        let doc = LoroDoc::new();
        crate::page_inline::configure(&doc);
        doc.import(snapshot).map_err(|e| CollabError::DecodeFailed {
            input: "snapshot",
            reason: e.to_string(),
        })?;
        let tree = Self::attach_tree(&doc);
        let meta = Self::attach_meta(&doc);
        let mut engine = Self {
            doc,
            tree,
            meta,
            id_to_tree: HashMap::new(),
            tree_to_id: HashMap::new(),
            uncached_tree_nodes: HashSet::new(),
            duplicate_logical_ids: false,
            id_cache_incremental: false,
            editor_model_floor: OnceLock::new(),
            session_index: None,
            session_stamp: None,
            #[cfg(test)]
            recompute_placements: false,
        };
        engine.rebuild_id_cache()?;
        Ok(engine)
    }

    fn import_update(&mut self, update: &[u8]) -> Result<Diff, Self::Error> {
        self.import_guarded(update, ImportMode::Replay { causal: false })
    }

    fn export_snapshot(&self) -> Result<Vec<u8>, Self::Error> {
        self.doc
            .export(loro::ExportMode::Snapshot)
            .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })
    }

    fn export_from(&self, frontier: &Frontier) -> Result<Vec<u8>, Self::Error> {
        let vv = if frontier.is_empty() {
            VersionVector::default()
        } else {
            VersionVector::decode(frontier.as_bytes()).map_err(|e| CollabError::DecodeFailed {
                input: "frontier",
                reason: e.to_string(),
            })?
        };
        self.doc
            .export(loro::ExportMode::updates(&vv))
            .map_err(|e| CollabError::OperationFailed { reason: e.to_string() })
    }

    fn frontier(&self) -> Frontier {
        Frontier::from_bytes(encode_frontier_entries(&state_entries(&self.doc.state_vv())))
    }
}

/// A version vector as a peer-ordered map, independent of Loro's hash-map layout.
fn state_entries(version: &VersionVector) -> BTreeMap<u64, i32> {
    version.iter().map(|(peer, counter)| (*peer, *counter)).collect()
}

/// Decodes Loro's postcard version-vector wire format (`varint len`, then `len` pairs of
/// `varint peer`, `zigzag varint counter`) in any entry order.
fn decode_frontier_entries(wire: &[u8]) -> Result<BTreeMap<u64, i32>, CollabError> {
    let malformed = |reason: &str| CollabError::DecodeFailed {
        input: "frontier",
        reason: reason.to_owned(),
    };
    if wire.is_empty() {
        return Ok(BTreeMap::new());
    }
    let (entries, remaining) =
        postcard::take_from_bytes::<Vec<(u64, i32)>>(wire).map_err(|e| malformed(&e.to_string()))?;
    if !remaining.is_empty() {
        return Err(malformed("trailing bytes after version vector"));
    }
    if !postcard::to_allocvec(&entries).is_ok_and(|encoded| encoded == wire) {
        return Err(malformed("non-minimal version vector encoding"));
    }
    let count = entries.len();
    let decoded: BTreeMap<u64, i32> = entries.into_iter().collect();
    if decoded.len() != count {
        return Err(malformed("duplicate peer in version vector"));
    }
    Ok(decoded)
}

/// Encodes a version vector in canonical, peer-sorted order. The bytes are wire-compatible with
/// `VersionVector::encode`/`decode` (a postcard map and a postcard sequence of pairs share one
/// layout), so canonical frontiers remain readable by Loro and by older releases.
fn encode_frontier_entries(entries: &BTreeMap<u64, i32>) -> Vec<u8> {
    fn push_varint(out: &mut Vec<u8>, mut value: u64) {
        // `to_le_bytes()[0]` is the low byte: the low seven bits plus the continuation flag.
        while value >= 0x80 {
            out.push(value.to_le_bytes()[0] | 0x80);
            value >>= 7;
        }
        out.push(value.to_le_bytes()[0]);
    }
    let mut out = Vec::with_capacity(1 + entries.len() * 16);
    push_varint(&mut out, entries.len() as u64);
    for (peer, counter) in entries {
        push_varint(&mut out, *peer);
        // postcard zigzag-encodes signed integers.
        push_varint(&mut out, u64::from(((counter << 1) ^ (counter >> 31)).cast_unsigned()));
    }
    out
}

/// Whether the harness removed the sequential gate, for the mutants of the differential harness.
// Not `const`: under test it reads the thread's injected fault.
#[allow(clippy::missing_const_for_fn)]
fn injected_fault_sequential_gate_removed() -> bool {
    #[cfg(test)]
    {
        crate::window_check::fault::active() == Some(crate::window_check::fault::Fault::SequentialGateRemoved)
    }
    #[cfg(not(test))]
    {
        false
    }
}

#[cfg(test)]
mod floor_equivalence_tests;
#[cfg(test)]
mod hand_encoded;
#[cfg(test)]
mod next_oracle;
#[cfg(test)]
mod placement_session_tests;
#[cfg(test)]
mod prewarm_tests;
#[cfg(test)]
mod read_side_effect_tests;
#[cfg(test)]
mod reserved_root_tests;
#[cfg(test)]
mod root_read_cost_tests;
#[cfg(test)]
mod window_calibration_tests;
#[cfg(test)]
mod window_check_pins_tests;
#[cfg(test)]
mod window_class_fixtures;
#[cfg(test)]
mod window_decision_tests;
#[cfg(test)]
mod window_differential_tests;
#[cfg(test)]
mod window_fixtures;
#[cfg(test)]
mod window_regression_tests;
#[cfg(test)]
mod window_round2_tests;
#[cfg(test)]
mod window_round3_tests;
#[cfg(test)]
mod window_round4_tests;
#[cfg(test)]
mod window_round6_tests;
#[cfg(test)]
mod window_setup_guard_tests;
#[cfg(test)]
pub(crate) mod window_text_guard_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn legacy_flat_page_initialization_preserves_alias_and_never_overwrites_existing_editor() {
        let mut engine = LoroCollabEngine::new_empty(42);
        let id: NodeId = "old-logical-block".into();
        engine
            .apply_operation(&Operation::CreateNode {
                id: id.clone(),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            })
            .unwrap();
        engine
            .apply_operation(&Operation::InsertText {
                id: id.clone(),
                index: 0,
                text: "Legacy 😀 prose".into(),
            })
            .unwrap();
        engine
            .apply_operation(&Operation::SetProperty {
                id: id.clone(),
                key: "block_type".into(),
                value: "heading".into(),
            })
            .unwrap();
        engine
            .apply_operation(&Operation::SetProperty {
                id: id.clone(),
                key: "level".into(),
                value: "2".into(),
            })
            .unwrap();
        engine
            .apply_operation(&Operation::CreateNode {
                id: "aaa-after".into(),
                parent: None,
                index: 1,
                kind: NodeKind::Block,
            })
            .unwrap();
        engine
            .apply_operation(&Operation::InsertText {
                id: "aaa-after".into(),
                index: 0,
                text: "second paragraph".into(),
            })
            .unwrap();
        assert!(engine.initialize_page_editor().unwrap());
        let canonical = engine.resolve_content_node_id(&id).unwrap();
        let migrated = engine.semantic_snapshot().unwrap();
        let first = migrated.nodes.get(&canonical).unwrap();
        let second = migrated
            .nodes
            .get(&engine.resolve_content_node_id(&NodeId::from("aaa-after")).unwrap())
            .unwrap();
        assert!(first.order_key < second.order_key);
        assert_eq!(first.properties.get("level").unwrap(), "2");

        assert!(canonical.starts_with("cid:"));
        assert_eq!(
            engine.semantic_snapshot().unwrap().nodes.get(&canonical).unwrap().text,
            "Legacy 😀 prose"
        );
        engine
            .apply_content_operation(&Operation::InsertText {
                id,
                index: u32::try_from("Legacy 😀".len()).unwrap(),
                text: " agent".into(),
            })
            .unwrap();
        assert_eq!(
            engine.semantic_snapshot().unwrap().nodes.get(&canonical).unwrap().text,
            "Legacy 😀 agent prose"
        );
        let before = engine.frontier();
        assert!(!engine.initialize_page_editor().unwrap());
        assert_eq!(engine.frontier(), before);
        if let Ok(output) = std::env::var("FLOW_EDITOR_BRIDGE_INITIALIZED_RUST_OUTPUT") {
            std::fs::write(output, engine.export_snapshot().unwrap()).unwrap();
        }
        let mut nested = LoroCollabEngine::new_empty(43);
        nested
            .apply_operation(&Operation::CreateNode {
                id: "parent".into(),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            })
            .unwrap();
        nested
            .apply_operation(&Operation::CreateNode {
                id: "child".into(),
                parent: Some("parent".into()),
                index: 0,
                kind: NodeKind::Block,
            })
            .unwrap();
        let frontier = nested.frontier();
        assert!(nested.initialize_page_editor().is_err());
        assert_eq!(nested.frontier(), frontier);
    }

    #[test]
    fn actual_browser_snapshot_accepts_cid_utf8_text_edit_and_preserves_anchor() {
        let bytes = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../frontend/tests/fixtures/editor-doc-v1/browser.snapshot"
        ))
        .unwrap();
        let mut engine = LoroCollabEngine::load(&bytes).unwrap();
        let before = engine.semantic_snapshot().unwrap();
        let (id, node) = before
            .nodes
            .iter()
            .find(|(_, node)| node.text == "Browser 😀 prose")
            .unwrap();
        let id = id.clone();
        let properties = node.properties.clone();
        assert!(id.starts_with("cid:"));
        engine
            .apply_operation(&Operation::InsertText {
                id: id.clone(),
                index: u32::try_from("Browser 😀".len()).unwrap(),
                text: " agent".into(),
            })
            .unwrap();
        let after = engine.semantic_snapshot().unwrap();
        let edited = after.nodes.get(&id).unwrap();
        assert_eq!(edited.text, "Browser 😀 agent prose");
        assert_eq!(edited.properties, properties);
        let snapshot = engine.export_snapshot().unwrap();
        let reopened = LoroCollabEngine::load(&snapshot).unwrap();
        assert_eq!(
            reopened.semantic_snapshot().unwrap().nodes.get(&id).unwrap().text,
            "Browser 😀 agent prose"
        );
        if let Ok(output) = std::env::var("FLOW_EDITOR_BRIDGE_RUST_OUTPUT") {
            std::fs::write(output, &snapshot).unwrap();
        }
    }

    #[test]
    fn browser_doc_refuses_shadow_tree_text_without_changing_either_frontier_or_prose() {
        let mut engine = LoroCollabEngine::new_empty(7);
        let id: NodeId = "legacy-prose".into();
        engine
            .apply_operation(&Operation::CreateNode {
                id: id.clone(),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            })
            .unwrap();
        engine
            .apply_operation(&Operation::InsertText {
                id: id.clone(),
                index: 0,
                text: "old tree".into(),
            })
            .unwrap();
        let children = engine
            .doc
            .get_map("doc")
            .insert_container("children", loro::LoroList::new())
            .unwrap();
        let block = children.insert_container(0, LoroMap::new()).unwrap();
        block.insert("nodeName", "paragraph").unwrap();
        let inline = block.insert_container("children", loro::LoroList::new()).unwrap();
        inline
            .insert_container(0, LoroText::new())
            .unwrap()
            .insert(0, "browser prose")
            .unwrap();
        engine.doc.commit();
        let before = engine.frontier();
        let semantic = engine.semantic_snapshot().unwrap();
        for operation in [
            Operation::InsertText {
                id: id.clone(),
                index: 0,
                text: "invisible".into(),
            },
            Operation::DeleteText { id, index: 0, len: 1 },
        ] {
            let err = engine.apply_operation(&operation).unwrap_err();
            assert!(err.to_string().contains("editor_doc_requires_editor_block_command"));
            assert_eq!(engine.frontier(), before);
            assert_eq!(engine.semantic_snapshot().unwrap(), semantic);
        }
    }

    #[test]
    fn zero_padded_order_key_matches_the_format_macro_it_replaced() {
        for position in [
            0usize,
            1,
            7,
            8,
            9,
            99,
            9_999,
            10_000,
            99_999_999,
            100_000_000,
            123_456_789,
        ] {
            assert_eq!(zero_padded_order_key(position), format!("{position:08}"));
        }
    }

    #[test]
    fn new_empty_has_no_title_and_empty_semantic_snapshot() {
        let engine = LoroCollabEngine::new_empty(1);
        assert_eq!(engine.title().expect("title reads"), "");
        assert!(engine.semantic_snapshot().expect("snapshot reads").nodes.is_empty());
    }

    #[test]
    fn set_title_round_trips_through_snapshot_export_and_load() {
        let mut engine = LoroCollabEngine::new_empty(1);
        engine.set_title("Untitled Page").expect("set_title succeeds");
        assert_eq!(engine.title().expect("title reads"), "Untitled Page");

        let snapshot = engine.export_snapshot().expect("export succeeds");
        let reloaded = LoroCollabEngine::load(&snapshot).expect("load succeeds");
        assert_eq!(reloaded.title().expect("title reads"), "Untitled Page");
    }

    #[test]
    fn full_snapshot_can_fork_back_to_an_earlier_semantic_frontier() {
        let mut engine = LoroCollabEngine::new_empty(1);
        engine.set_title("before").expect("initial title writes");
        let before = engine.frontier();
        engine.set_title("after").expect("second title writes");
        engine
            .apply_operation(&Operation::CreateNode {
                id: NodeId::from("later-node"),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            })
            .expect("later node writes");

        let full = engine.export_snapshot().expect("full snapshot exports");
        let loaded = LoroCollabEngine::load(&full).expect("full snapshot loads");
        let historical = loaded
            .fork_at_frontier(&before)
            .expect("historical frontier is retained");

        assert_eq!(historical.title().expect("title reads"), "before");
        assert!(
            historical.semantic_snapshot().expect("snapshot reads").nodes.is_empty(),
            "the fork must not contain an operation that happened after its frontier"
        );
        assert_eq!(loaded.title().expect("source engine stays current"), "after");
    }

    #[test]
    fn revert_to_frontier_appends_a_forward_change_and_keeps_the_later_version() {
        let mut author = LoroCollabEngine::new_empty(3);
        author.set_title("v1").expect("first title");
        let v1 = author.frontier();
        author.set_title("v2").expect("second title");
        let v2 = author.frontier();
        let snapshot = author.export_snapshot().expect("snapshot at v2");

        author.revert_to_frontier(&v1).expect("revert appends inverse ops");
        assert_eq!(author.title().expect("reverted title"), "v1");
        assert_ne!(author.frontier(), v1, "revert must not rewind the head frontier");
        assert_ne!(author.frontier(), v2);
        let still_there = author.fork_at_frontier(&v2).expect("v2 stays in history");
        assert_eq!(still_there.title().expect("historical title"), "v2");

        let mut peer = LoroCollabEngine::load(&snapshot).expect("peer loads v2");
        peer.apply_operation(&Operation::CreateNode {
            id: NodeId::from("concurrent"),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        })
        .expect("concurrent insert");
        let peer_delta = peer.export_from(&v2).expect("peer delta");
        let revert_delta = author.export_from(&v2).expect("revert delta");
        peer.import_update(&revert_delta).expect("peer imports revert");
        author
            .import_update(&peer_delta)
            .expect("author imports concurrent insert");

        assert_eq!(peer.title().expect("merged peer title"), "v1");
        assert_eq!(author.title().expect("merged author title"), "v1");
        assert!(
            peer.semantic_snapshot()
                .expect("peer snapshot")
                .nodes
                .contains_key(&NodeId::from("concurrent")),
            "a concurrent insert survives the imported revert"
        );
        assert!(
            author
                .semantic_snapshot()
                .expect("author snapshot")
                .nodes
                .contains_key(&NodeId::from("concurrent")),
            "both replicas converge on the concurrent insert"
        );
    }

    #[test]
    fn root_state_json_exposes_the_title_and_drops_binary_values() {
        let mut engine = LoroCollabEngine::new_empty(4);
        engine.set_title("Visible").expect("title");
        let state = engine.root_state_json();
        assert_eq!(
            state
                .get("meta")
                .and_then(|meta| meta.get("title"))
                .and_then(Value::as_str),
            Some("Visible")
        );
    }

    #[test]
    fn replay_frontier_accepts_peer_order_only_and_rejects_malformed_or_different_vectors() {
        let engine = LoroCollabEngine::new_empty(31);
        engine.doc.set_peer_id(31).unwrap();
        engine.doc.get_map("proof").insert("first", 1).unwrap();
        engine.doc.commit();
        engine.doc.set_peer_id(47).unwrap();
        engine.doc.get_map("proof").insert("second", 2).unwrap();
        engine.doc.commit();
        let version = engine.doc.state_vv();
        let first = (31_u64, *version.get(&31).unwrap());
        let second = (47_u64, *version.get(&47).unwrap());
        let forward = postcard::to_allocvec(&[first, second].as_slice()).unwrap();
        let reverse = postcard::to_allocvec(&[second, first].as_slice()).unwrap();
        assert_ne!(forward, reverse);
        let after = LoroCollabEngine::load(&engine.export_snapshot().unwrap()).unwrap();
        for state in [engine, after] {
            assert!(state.matches_frontier(&forward));
            assert!(state.matches_frontier(&reverse));
            assert!(!state.matches_frontier(&postcard::to_allocvec(&[first, first].as_slice()).unwrap()));
            assert!(
                !state.matches_frontier(&postcard::to_allocvec(&[first, (47_u64, second.1 + 1)].as_slice()).unwrap())
            );
            let mut trailing = reverse.clone();
            trailing.push(0);
            assert!(!state.matches_frontier(&trailing));
            assert!(!state.matches_frontier(&[2]));
        }
    }

    #[test]
    fn frontier_is_canonical_peer_sorted_and_loro_wire_compatible() {
        let engine = LoroCollabEngine::new_empty(1);
        for peer in [u64::MAX - 3, 47, 0x8000_0000_0000_0000, 31, 2] {
            engine.doc.set_peer_id(peer).unwrap();
            for i in 0..3 {
                engine.doc.get_map("proof").insert(&format!("{peer}-{i}"), 1).unwrap();
                engine.doc.commit();
            }
        }
        let frontier = engine.frontier();
        let version = engine.doc.state_vv();
        let mut sorted: Vec<(u64, i32)> = version.iter().map(|(p, c)| (*p, *c)).collect();
        sorted.sort_unstable();
        // Byte-identical to postcard's encoding of the sorted entries ...
        assert_eq!(frontier.as_bytes(), postcard::to_allocvec(&sorted).unwrap().as_slice());
        // ... and still decodable by Loro, naming the same version.
        assert_eq!(VersionVector::decode(frontier.as_bytes()).unwrap(), version);
        // Loro's own iteration-order encoding is equivalent but need not be byte-identical.
        assert!(LoroCollabEngine::frontiers_equivalent(frontier.as_bytes(), &version.encode()).unwrap());
        assert_eq!(
            LoroCollabEngine::canonical_frontier(&version.encode()).unwrap(),
            frontier
        );
        assert!(engine.matches_frontier(frontier.as_bytes()));
    }

    #[test]
    fn frontiers_equivalent_compares_versions_and_rejects_undecodable_input() {
        let first = (31_u64, 4_i32);
        let second = (47_u64, 9_i32);
        let canonical = postcard::to_allocvec(&[first, second].as_slice()).unwrap();
        let legacy_order = postcard::to_allocvec(&[second, first].as_slice()).unwrap();
        let advanced = postcard::to_allocvec(&[first, (47_u64, 10_i32)].as_slice()).unwrap();
        let fewer = postcard::to_allocvec(&[first].as_slice()).unwrap();

        assert!(LoroCollabEngine::frontiers_equivalent(&canonical, &legacy_order).unwrap());
        assert!(LoroCollabEngine::frontiers_equivalent(&legacy_order, &canonical).unwrap());
        assert!(!LoroCollabEngine::frontiers_equivalent(&canonical, &advanced).unwrap());
        assert!(!LoroCollabEngine::frontiers_equivalent(&legacy_order, &fewer).unwrap());
        // The empty byte string is the empty version (`compaction_boundary_frontier` default).
        assert!(LoroCollabEngine::frontiers_equivalent(&[], &[0]).unwrap());
        assert!(!LoroCollabEngine::frontiers_equivalent(&[], &canonical).unwrap());

        let mut trailing = legacy_order;
        trailing.push(0);
        let duplicate = postcard::to_allocvec(&[first, first].as_slice()).unwrap();
        // 0x80 0x00 is a non-minimal varint for zero entries.
        for malformed in [
            trailing.as_slice(),
            duplicate.as_slice(),
            &[2],
            &[0x80, 0x00],
            b"fenced",
        ] {
            for (left, right) in [
                (malformed, canonical.as_slice()),
                (canonical.as_slice(), malformed),
                (malformed, malformed),
            ] {
                assert!(
                    matches!(
                        LoroCollabEngine::frontiers_equivalent(left, right),
                        Err(CollabError::DecodeFailed { input: "frontier", .. })
                    ),
                    "{left:?} vs {right:?}"
                );
            }
            assert!(LoroCollabEngine::canonical_frontier(malformed).is_err());
        }
    }

    #[test]
    fn frontier_is_stable_for_an_untouched_document_and_advances_after_commit() {
        let engine = LoroCollabEngine::new_empty(7);
        let empty_frontier = engine.frontier();

        let mut engine = engine;
        engine.set_title("x").expect("set_title succeeds");
        let after_frontier = engine.frontier();
        assert_ne!(empty_frontier, after_frontier);
    }

    #[test]
    fn apply_operation_create_and_semantic_snapshot_agree() {
        let mut engine = LoroCollabEngine::new_empty(1);
        let block = NodeId::from("blk-1");
        engine
            .apply_operation(&Operation::CreateNode {
                id: block.clone(),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            })
            .expect("create succeeds");
        engine
            .apply_operation(&Operation::InsertText {
                id: block.clone(),
                index: 0,
                text: "hello".to_string(),
            })
            .expect("insert text succeeds");

        let snapshot = engine.semantic_snapshot().expect("snapshot reads");
        let node = snapshot.nodes.get(&block).expect("node exists");
        assert_eq!(node.text, "hello");
        assert_eq!(node.kind, NodeKind::Block);
        assert!(!node.deleted);
    }

    #[test]
    fn fork_is_independent_of_the_original() {
        let mut engine = LoroCollabEngine::new_empty(1);
        engine.set_title("before fork").expect("set_title succeeds");

        let mut forked = engine.fork().expect("fork succeeds");
        assert_eq!(forked.title().expect("title reads"), "before fork");

        forked
            .set_title("mutated only on the fork")
            .expect("set_title succeeds");
        assert_eq!(forked.title().expect("title reads"), "mutated only on the fork");
        // The original must be untouched -- this is the whole point of `fork` over `Clone`.
        assert_eq!(engine.title().expect("title reads"), "before fork");
        assert_ne!(engine.frontier(), forked.frontier());
    }

    /// An update whose causal dependencies are not in the document leaves Loro's import
    /// pending; it must be refused rather than reported as an (empty) success, and must leave the
    /// document unchanged. The same bytes import once their dependency is present.
    #[test]
    fn import_of_an_update_with_missing_causal_dependencies_is_refused() {
        let mut author = LoroCollabEngine::new_empty(1);
        author.set_title("first").expect("first edit");
        let first_only = author.export_from(&Frontier::default()).expect("first export");
        let after_first = author.frontier();
        author.set_title("second").expect("second edit");
        let dependent = author.export_from(&after_first).expect("dependent export");

        let mut receiver = LoroCollabEngine::new_empty(2);
        let before = receiver.frontier();
        let refused = receiver.import_update_causally_complete(&dependent);
        assert!(
            matches!(refused, Err(CollabError::CausalDependencyMissing)),
            "a causally incomplete update must be refused, got {refused:?}"
        );
        assert_eq!(
            receiver.frontier(),
            before,
            "a refused update leaves the document unchanged"
        );
        assert_eq!(receiver.title().expect("title reads"), "");

        let mut complete = LoroCollabEngine::new_empty(2);
        complete
            .import_update_causally_complete(&first_only)
            .expect("the dependency imports");
        complete
            .import_update_causally_complete(&dependent)
            .expect("the dependent update now imports");
        assert_eq!(complete.title().expect("title reads"), "second");
    }

    /// The memoized floor, if `minimum_editor_model` would use it right now, must equal a fresh
    /// walk of the same state (same value, or the same typed error).
    fn assert_floor_cache_matches_walk(engine: &LoroCollabEngine, step: &str) -> (Result<u32, CollabError>, bool) {
        let fresh = engine.compute_minimum_editor_model();
        let mut hit = false;
        if let Some(cached) = engine.editor_model_floor.get()
            && engine.doc.get_pending_txn_len() == 0
            && cached.state == engine.doc.state_frontiers()
        {
            hit = true;
            assert_eq!(Ok(cached.floor), fresh, "stale cached floor after {step}");
        }
        assert_eq!(
            engine.minimum_editor_model(),
            fresh,
            "minimum_editor_model disagrees with a fresh walk after {step}"
        );
        (fresh, hit)
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    const ASSET: &str = "00000000-0000-4000-8000-000000000001";

    fn image_attrs() -> Value {
        serde_json::json!({"asset_id": ASSET, "alt": "", "caption": "", "indent": 0})
    }

    /// A raw-Loro update that deletes every top-level editor block and rewrites the structure
    /// model to v4: the shape of a guest update trying to drop a model-5 floor.
    fn floor_dropping_update(engine: &LoroCollabEngine) -> Option<Vec<u8>> {
        let raw = LoroDoc::new();
        raw.import(&engine.export_snapshot().ok()?).ok()?;
        raw.set_peer_id(9_999).ok()?;
        let before = raw.oplog_vv();
        raw.get_map(crate::editor_structure::META)
            .insert("model", "rich_page_v4")
            .ok()?;
        if let Some(ValueOrContainer::Container(Container::List(list))) = raw.get_map("doc").get("children") {
            for index in (0..list.len()).rev() {
                if let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(index)
                    && matches!(map.get("nodeName"), Some(ValueOrContainer::Value(LoroValue::String(kind))) if crate::editor_media::is_atom(kind.as_ref()))
                {
                    list.delete(index, 1).ok()?;
                }
            }
        }
        raw.commit();
        raw.export(loro::ExportMode::updates(&before)).ok()
    }

    /// Rebuilds `engine`'s head from an older snapshot plus one tail update, the way the host
    /// rebuilds a document from storage. `None` when the tail does not import.
    fn rebuild_from_snapshot(
        engine: &LoroCollabEngine,
        frontier: &Frontier,
        snapshot: &[u8],
        label: &str,
    ) -> Option<LoroCollabEngine> {
        let tail = engine.export_from(frontier).unwrap();
        let mut rebuilt = LoroCollabEngine::load(snapshot).unwrap();
        let _ = assert_floor_cache_matches_walk(&rebuilt, &format!("{label} rebuild base"));
        rebuilt.import_update(&tail).ok().map(|_| rebuilt)
    }

    /// Cross-checks the memoized editor-model floor against a fresh walk after every step of
    /// randomized sequences of local edits, peer imports (lenient and causally complete, including
    /// rejected floor drops and causally incomplete updates), reverts, snapshot loads, forks, and
    /// snapshot-plus-tail rebuilds. A missed invalidation on any mutation path fails here.
    #[test]
    fn cached_editor_model_floor_matches_a_fresh_walk_across_random_histories() {
        let mut floors = std::collections::BTreeSet::new();
        let mut hits = 0;
        let mut floor_drops_rejected = 0;
        for seed in [
            0x9e37_79b9_7f4a_7c15_u64,
            0x2545_f491_4f6c_dd1d,
            0xdead_beef_cafe_f00d,
            7,
            42,
        ] {
            let mut rng = Rng(seed);
            let mut engine = LoroCollabEngine::new_empty(seed % 1_000);
            let mut peer = LoroCollabEngine::new_empty(seed % 1_000 + 1);
            let mut history: Vec<(Frontier, Vec<u8>)> = Vec::new();
            let mut ids: Vec<String> = Vec::new();
            let mut next_id = 0_u32;
            for step in 0..120 {
                let label = format!("seed {seed:#x} step {step}");
                if rng.below(3) == 0 {
                    // Populate the cache so later steps exercise hits as well as misses.
                    let _ = engine.minimum_editor_model();
                }
                match rng.below(16) {
                    0 => {
                        let _ = engine.initialize_page_editor();
                    }
                    1 => {
                        let _ = engine.initialize_page_structure_model(if rng.below(2) == 0 { 4 } else { 5 });
                    }
                    2 | 3 => {
                        let id = format!("blk-{next_id}");
                        next_id += 1;
                        let index = u32::try_from(rng.below(3)).unwrap();
                        if engine
                            .apply_content_operation(&Operation::CreateNode {
                                id: id.as_str().into(),
                                parent: None,
                                index,
                                kind: NodeKind::Block,
                            })
                            .is_ok()
                        {
                            ids.push(id);
                        }
                    }
                    4 => {
                        if let Some(id) = ids.get(usize::try_from(rng.below(ids.len().max(1) as u64)).unwrap()) {
                            let _ = engine.apply_content_operation(&Operation::InsertText {
                                id: id.as_str().into(),
                                index: 0,
                                text: "word ".into(),
                            });
                        }
                    }
                    5 => {
                        if let Some(id) = ids.get(usize::try_from(rng.below(ids.len().max(1) as u64)).unwrap()) {
                            let _ = engine.apply_content_operation(&Operation::DeleteNode { id: id.as_str().into() });
                        }
                    }
                    6 => {
                        if let Some(id) = ids.get(usize::try_from(rng.below(ids.len().max(1) as u64)).unwrap()) {
                            let _ = engine.apply_page_marks(
                                id,
                                &[crate::page_inline::MarkEdit {
                                    start: 0,
                                    end: 1,
                                    mark: crate::page_inline::InlineMark::Bold,
                                    clear: false,
                                    href: None,
                                }],
                            );
                        }
                    }
                    7 => {
                        let id = format!("atom-{next_id}");
                        next_id += 1;
                        let _ = engine.insert_page_atom(&id, None, 0, "image", &image_attrs());
                    }
                    8 => {
                        let _ = engine.set_title(&format!("title {step}"));
                        let _ = engine.set_navigator_order(&format!("nav-{step}"), "a0");
                    }
                    9 => {
                        if let Some((frontier, _)) =
                            history.get(usize::try_from(rng.below(history.len().max(1) as u64)).unwrap())
                        {
                            let frontier = frontier.clone();
                            let _ = engine.revert_to_frontier(&frontier);
                        }
                    }
                    10 | 11 => {
                        // A peer edit arrives as an update.
                        let _ = peer.import_update(&engine.export_from(&peer.frontier()).unwrap());
                        let _ = peer.initialize_page_editor();
                        if rng.below(2) == 0 {
                            let _ =
                                peer.insert_page_atom(&format!("peer-atom-{step}"), None, 0, "image", &image_attrs());
                        } else {
                            let _ = peer.apply_content_operation(&Operation::CreateNode {
                                id: format!("peer-{step}").as_str().into(),
                                parent: None,
                                index: 0,
                                kind: NodeKind::Block,
                            });
                        }
                        let update = peer.export_from(&engine.frontier()).unwrap();
                        let _ = if rng.below(2) == 0 {
                            engine.import_update(&update)
                        } else {
                            engine.import_update_causally_complete(&update)
                        };
                    }
                    12 => {
                        if let Some(update) = floor_dropping_update(&engine)
                            && matches!(
                                engine.import_update_causally_complete(&update),
                                Err(CollabError::OperationFailed { ref reason }) if reason == "editor_model_floor_downgrade"
                            )
                        {
                            floor_drops_rejected += 1;
                        }
                    }
                    13 => {
                        // A causally incomplete update: refused, or buffered by the lenient path.
                        let mut author = engine.fork().unwrap();
                        author.set_title("dependency").unwrap();
                        let after = author.frontier();
                        author.set_title("dependent").unwrap();
                        let dependent = author.export_from(&after).unwrap();
                        let _ = if rng.below(2) == 0 {
                            engine.import_update(&dependent)
                        } else {
                            engine.import_update_causally_complete(&dependent)
                        };
                    }
                    14 => {
                        engine = match rng.below(3) {
                            0 => LoroCollabEngine::load(&engine.export_snapshot().unwrap()).unwrap(),
                            1 => engine.fork().unwrap(),
                            _ => history
                                .last()
                                .and_then(|(frontier, _)| engine.fork_at_frontier(frontier).ok())
                                .unwrap_or_else(|| engine.fork().unwrap()),
                        };
                    }
                    _ => {
                        // Host-side rebuild: an older snapshot plus the tail since then.
                        if let Some(rebuilt) = history
                            .first()
                            .and_then(|(frontier, snapshot)| rebuild_from_snapshot(&engine, frontier, snapshot, &label))
                        {
                            engine = rebuilt;
                        }
                    }
                }
                let (floor, hit) = assert_floor_cache_matches_walk(&engine, &label);
                floors.insert(floor.map_err(|error| error.to_string()));
                hits += usize::from(hit);
                if step % 10 == 0 {
                    history.push((engine.frontier(), engine.export_snapshot().unwrap()));
                }
            }
        }
        // The sequences must actually reach every floor and exercise cache hits and rejections.
        for floor in [1, 4, 5] {
            assert!(floors.contains(&Ok(floor)), "floor {floor} never reached: {floors:?}");
        }
        assert!(hits > 10, "too few cache hits exercised: {hits}");
        assert!(floor_drops_rejected > 0, "no floor drop was exercised");
    }

    fn model5_page_with_tail(blocks: usize, tail: usize) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut engine = LoroCollabEngine::new_empty(61);
        engine.initialize_page_editor().unwrap();
        engine.initialize_page_structure_model(5).unwrap();
        engine
            .insert_page_atom("image", None, 0, "image", &image_attrs())
            .unwrap();
        for index in 0..blocks {
            engine
                .apply_content_operation(&Operation::CreateNode {
                    id: format!("b{index}").as_str().into(),
                    parent: None,
                    index: 0,
                    kind: NodeKind::Block,
                })
                .unwrap();
        }
        assert_eq!(engine.minimum_editor_model().unwrap(), 5);
        let snapshot = engine.export_snapshot().unwrap();
        let mut updates = Vec::new();
        for index in 0..tail {
            let before = engine.frontier();
            engine
                .apply_content_operation(&Operation::CreateNode {
                    id: format!("t{index}").as_str().into(),
                    parent: None,
                    index: 0,
                    kind: NodeKind::Block,
                })
                .unwrap();
            updates.push(engine.export_from(&before).unwrap());
        }
        (snapshot, updates)
    }

    /// Regression guard (count-based, not wall clock): replaying a tail of k updates onto a
    /// model-5 snapshot walks the editor-model floor zero times (the downgrade guard reads only
    /// the marker and the imported ops), and the head state's floor costs one walk on first read.
    #[test]
    fn rebuild_tail_does_not_walk_the_floor() {
        let tail = 6;
        let (snapshot, updates) = model5_page_with_tail(4, tail);
        let mut engine = LoroCollabEngine::load(&snapshot).unwrap();
        crate::walk_counter::reset();
        for update in &updates {
            engine.import_update(update).unwrap();
        }
        assert_eq!(crate::walk_counter::counts().0, 0);
        assert_eq!(engine.minimum_editor_model().unwrap(), 5);
        assert_eq!(crate::walk_counter::counts().0, 1);
        assert_eq!(engine.minimum_editor_model().unwrap(), 5);
        assert_eq!(crate::walk_counter::counts().0, 1, "the second read is memoized");
    }

    /// Regression guard (count-based, not wall clock) for the isolated-apply worker's metered
    /// window -- `import_update_causally_complete` then `semantic_snapshot_with_floor` on a
    /// freshly loaded base -- and for a later import onto the same engine, on model-1 and model-5
    /// pages: no floor walk at all, one prose read for the snapshot, and the floor of the
    /// imported state is known afterwards without a further walk.
    #[test]
    fn worker_window_walks_are_bounded() {
        for model5 in [false, true] {
            let mut author = LoroCollabEngine::new_empty(71);
            author.initialize_page_editor().unwrap();
            if model5 {
                author.initialize_page_structure_model(5).unwrap();
                author
                    .insert_page_atom("image", None, 0, "image", &image_attrs())
                    .unwrap();
            }
            for index in 0..20 {
                author
                    .apply_content_operation(&Operation::CreateNode {
                        id: format!("b{index}").as_str().into(),
                        parent: None,
                        index: 0,
                        kind: NodeKind::Block,
                    })
                    .unwrap();
            }
            let base = author.export_snapshot().unwrap();
            let mut updates = Vec::new();
            for index in 0..2 {
                let before = author.frontier();
                author
                    .apply_content_operation(&Operation::InsertText {
                        id: format!("b{index}").as_str().into(),
                        index: 0,
                        text: "typed".into(),
                    })
                    .unwrap();
                updates.push(author.export_from(&before).unwrap());
            }
            let mut engine = LoroCollabEngine::load(&base).unwrap();
            crate::walk_counter::reset();
            engine
                .import_update_causally_complete(updates.first().unwrap())
                .unwrap();
            let (_, floor) = engine.semantic_snapshot_with_floor().unwrap();
            assert_eq!(
                crate::walk_counter::counts(),
                (0, 1),
                "model5={model5}: (floor walks, block reads) in the worker window"
            );
            assert_eq!(floor, if model5 { 5 } else { 1 });
            assert_eq!(engine.minimum_editor_model().unwrap(), floor);
            assert_eq!(crate::walk_counter::counts().0, 0, "the fused floor filled the memo");

            crate::walk_counter::reset();
            engine.import_update_causally_complete(updates.get(1).unwrap()).unwrap();
            assert_eq!(
                crate::walk_counter::counts().0,
                0,
                "model5={model5}: floor walks for a follow-up import"
            );
        }
    }
}
