//! Native tree placement index over immutable physical editor block containers.
use crate::CollabError;
pub use crate::reserved_roots::{META, TREE};
use loro::{
    Container, ContainerID, ContainerTrait, ContainerType, LoroDoc, LoroMap, LoroTree, LoroValue, TreeID, TreeParentId,
    ValueOrContainer,
};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
fn invalid(reason: &str) -> CollabError {
    CollabError::OperationFailed { reason: reason.into() }
}
fn value(map: &LoroMap, key: &str) -> Option<String> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(value))) => Some(value.to_string()),
        _ => None,
    }
}
/// The root container `name` of exactly type `kind`, or `None` when no such root exists. The read
/// changes nothing: no root is registered, so `get_deep_value`, snapshots and exports are the
/// same afterwards.
///
/// Loro treats every root id as existing (`has_container` is true for it), and a typed getter
/// (`get_container`, `get_map`, ...) registers an empty root of that type when the document has
/// none, which shows up in `get_deep_value`, in snapshot bytes and in the list of roots (the
/// frontier and the semantic hash do not change). So the typed getter is only called once the
/// root is known to exist:
///
/// 1. `LoroDoc::get_by_path` with the single key `name` finds the registered root of that name
///    (the root the library prefers when several share it) without touching any other root. A
///    name that is absent has no root of any type: `None`. `LoroDoc::get_value` must not be used
///    for this: it values every root, so its cost grows with the whole document (the editor tree
///    of a 5,000-block page costs milliseconds per call), and this read runs several times per
///    write inside the isolated worker's CPU window.
/// 2. The found root is of type `kind`: it exists, and the typed getter finds it registered.
/// 3. The found root is of another type. The path lookup shows one root per name, so a root of
///    `kind` may still exist beside it; the document is a fork-and-compare away from knowing
///    (O(document), only for a document that has a same-named root of another type): the typed
///    getter runs on a fork, and the root exists exactly when that did not change the fork's
///    snapshot.
///
/// The typed id is what makes the read exact. The path lookup alone must not be used to read a
/// root: it picks among all roots with that name, of any type, so a second root of another type
/// could make two readers of one document disagree. It is only the existence probe of step 1.
///
/// The first lookup on a document loaded from a snapshot lists the keys of the store once
/// (O(containers), the library's lazy root scan); the lookups after it cost the named root only.
///
/// # Errors
/// `editor_root_probe_failed` when the fork-and-compare of step 3 cannot export the document. The
/// root is then neither present nor absent as far as this read knows; the caller must not assume
/// absence.
pub(crate) fn root_container(doc: &LoroDoc, name: &str, kind: ContainerType) -> Result<Option<Container>, CollabError> {
    #[cfg(test)]
    let allocated_before = crate::test_alloc::allocated_bytes();
    let found = probe_root(doc, name, kind);
    #[cfg(test)]
    crate::walk_counter::record_root_probe(
        name,
        crate::test_alloc::allocated_bytes().saturating_sub(allocated_before),
    );
    found
}
fn probe_root(doc: &LoroDoc, name: &str, kind: ContainerType) -> Result<Option<Container>, CollabError> {
    let id = ContainerID::new_root(name, kind);
    match doc.get_by_path(&[loro::Index::Key(name.into())]) {
        None => return Ok(None),
        Some(ValueOrContainer::Container(found)) if found.get_type() == kind => {}
        Some(_) => {
            #[cfg(test)]
            crate::walk_counter::record_root_fork();
            let probe = |error: loro::LoroEncodeError| invalid(&format!("editor_root_probe_failed: {error}"));
            let fork = doc.fork();
            let before = fork.export(loro::ExportMode::Snapshot).map_err(probe)?;
            if fork.get_container(id.clone()).is_none() {
                return Ok(None);
            }
            if fork.export(loro::ExportMode::Snapshot).map_err(probe)? != before {
                return Ok(None);
            }
        }
    }
    Ok(doc.get_container(id))
}
/// The reserved root `name` (see [`crate::reserved_roots`]) with its one allowed container type,
/// read exactly as [`root_container`] reads it.
///
/// # Errors
/// As [`root_container`]; `editor_root_not_reserved` for a name that is not in the table.
pub(crate) fn reserved_root(doc: &LoroDoc, name: &str) -> Result<Option<Container>, CollabError> {
    let kind = crate::reserved_roots::expected_type(name).ok_or_else(|| invalid("editor_root_not_reserved"))?;
    root_container(doc, name, kind)
}
/// [`reserved_root`] when the root is a map (`None` also when it is absent).
pub(crate) fn root_map(doc: &LoroDoc, name: &str) -> Result<Option<LoroMap>, CollabError> {
    Ok(match reserved_root(doc, name)? {
        Some(Container::Map(map)) => Some(map),
        _ => None,
    })
}
pub fn physical(doc: &LoroDoc) -> Result<Vec<LoroMap>, CollabError> {
    let Some(Container::Map(root)) = reserved_root(doc, crate::reserved_roots::DOC)? else {
        return Err(invalid("editor_doc_required"));
    };
    let Some(ValueOrContainer::Container(Container::List(list))) = root.get("children") else {
        return Err(invalid("editor_doc_required"));
    };
    (0..list.len())
        .map(|index| match list.get(index) {
            Some(ValueOrContainer::Container(Container::Map(map))) => Ok(map),
            _ => Err(invalid("editor_doc_unsupported_block_container")),
        })
        .collect()
}
/// Whether the structure metadata root says the placement index is initialised.
///
/// # Errors
/// As [`root_container`].
pub fn ready(doc: &LoroDoc) -> Result<bool, CollabError> {
    let Some(Container::Map(meta)) = reserved_root(doc, META)? else {
        return Ok(false);
    };
    Ok(matches!(
        meta.get("initialized"),
        Some(ValueOrContainer::Value(LoroValue::Bool(true)))
    ))
}
fn tree(doc: &LoroDoc) -> Result<LoroTree, CollabError> {
    let Some(Container::Tree(tree)) = reserved_root(doc, TREE)? else {
        return Err(invalid("editor_structure_missing_tree"));
    };
    tree.enable_fractional_index(0);
    Ok(tree)
}
#[derive(Debug, Clone)]
pub struct Placement {
    pub cid: String,
    pub parent: Option<String>,
    pub depth: u32,
    pub tree_id: TreeID,
    pub position: usize,
}
pub fn placements(doc: &LoroDoc) -> Result<Option<Vec<Placement>>, CollabError> {
    #[cfg(test)]
    full_walks::record();
    if !ready(doc)? {
        return Ok(None);
    }
    let meta = doc.get_map(META);
    if !matches!(
        meta.get("version"),
        Some(ValueOrContainer::Value(LoroValue::I64(1) | LoroValue::Double(1.0)))
    ) || !matches!(value(&meta, "model").as_deref(), Some("rich_page_v4" | "rich_page_v5"))
    {
        return Err(invalid("editor_structure_model_unsupported"));
    }
    let tree = tree(doc)?;
    let maps = physical(doc)?;
    let physical: BTreeSet<_> = maps.iter().map(|map| map.id().to_string()).collect();
    let mut all = BTreeSet::new();
    // Every node's `block_cid`, read once here and reused for the active traversal below.
    let mut cids = std::collections::HashMap::new();
    for node in tree.nodes() {
        let Some(cid) = value(&tree.get_meta(node).map_err(|e| invalid(&e.to_string()))?, "block_cid") else {
            continue;
        };
        cids.insert(node, cid.clone());
        let known = physical.contains(&cid)
            || loro::ContainerID::try_from(cid.as_str())
                .ok()
                .and_then(|id| doc.get_container(id))
                .is_some_and(|container| matches!(container, Container::Map(_)));
        if !known {
            return Err(invalid("editor_structure_invalid_block_cid"));
        }
        all.insert(cid);
    }
    if physical.iter().any(|cid| !all.contains(cid)) {
        return Err(invalid("editor_structure_unindexed_physical_block"));
    }
    let output = active_placements_with(&tree, |node| {
        cids.get(&node)
            .map_or_else(|| block_cid(&tree, node), |cid| Ok(Some(cid.clone())))
    })?;
    if output.iter().any(|entry| !physical.contains(&entry.cid)) {
        return Err(invalid("editor_structure_active_block_not_in_storage"));
    }
    Ok(Some(output))
}
fn block_cid(tree: &LoroTree, node: TreeID) -> Result<Option<String>, CollabError> {
    Ok(value(
        &tree.get_meta(node).map_err(|e| invalid(&e.to_string()))?,
        "block_cid",
    ))
}
fn active_placements(tree: &LoroTree) -> Result<Vec<Placement>, CollabError> {
    #[cfg(test)]
    full_walks::record();
    active_placements_with(tree, |node| block_cid(tree, node))
}
/// The active (non-deleted) placements in document order. `cid_of` returns a node's
/// `block_cid` exactly as [`block_cid`] would (it may answer from values already read).
fn active_placements_with(
    tree: &LoroTree,
    cid_of: impl Fn(TreeID) -> Result<Option<String>, CollabError>,
) -> Result<Vec<Placement>, CollabError> {
    let mut output = Vec::new();
    let mut active = BTreeSet::new();
    let mut stack: Vec<_> = tree
        .roots()
        .into_iter()
        .enumerate()
        .rev()
        .map(|(index, node)| (node, None, 0, index))
        .collect();
    while let Some((node, parent, depth, position)) = stack.pop() {
        if depth > 7 {
            return Err(invalid("editor_structure_depth_limit"));
        }
        let cid = cid_of(node)?.ok_or_else(|| invalid("editor_structure_missing_block_cid"))?;
        if !active.insert(cid.clone()) {
            return Err(invalid("editor_structure_duplicate_active_block_cid"));
        }
        output.push(Placement {
            cid: cid.clone(),
            parent,
            depth,
            tree_id: node,
            position,
        });
        for (index, child) in tree.children(node).unwrap_or_default().into_iter().enumerate().rev() {
            stack.push((child, Some(cid.clone()), depth + 1, index));
        }
    }
    if output.is_empty() {
        return Err(invalid("editor_doc_requires_one_block"));
    }
    Ok(output)
}
pub fn initialize(doc: &LoroDoc) -> Result<bool, CollabError> {
    if ready(doc)? {
        placements(doc)?;
        return Ok(false);
    }
    if reserved_root(doc, TREE)?.is_some_and(|value| match value {
        Container::Tree(tree) => !tree.roots().is_empty(),
        _ => true,
    }) {
        return Err(invalid("editor_structure_partial_initialization"));
    }
    let maps = physical(doc)?;
    let readable = crate::editor_doc_bridge::editor_blocks(doc)?.ok_or_else(|| invalid("editor_doc_required"))?;
    if readable.len() != maps.len() || maps.len() > crate::limits::DOCUMENT_BLOCK_COUNT_MAX {
        return Err(invalid("editor_structure_unsupported_physical_block"));
    }
    let mut depths = Vec::new();
    for map in &maps {
        let depth = match map.get("attributes") {
            Some(ValueOrContainer::Container(Container::Map(attrs))) => match attrs.get("indent") {
                Some(ValueOrContainer::Value(LoroValue::I64(value))) => {
                    u32::try_from(value).map_err(|_| invalid("editor_structure_invalid_indent"))?
                }
                Some(ValueOrContainer::Value(LoroValue::Double(value)))
                    if value.fract() == 0.0 && (0.0..=7.0).contains(&value) =>
                {
                    (0..=7)
                        .find(|depth| f64::from(*depth).to_bits() == value.to_bits())
                        .ok_or_else(|| invalid("editor_structure_invalid_indent"))?
                }
                _ => 0,
            },
            _ => 0,
        };
        if depth > 7 || depths.last().is_none_or(|previous| depth > previous + 1) && depth != 0 {
            return Err(invalid("editor_structure_invalid_indent"));
        }
        depths.push(depth);
    }
    if maps.is_empty() {
        return Err(invalid("editor_doc_requires_one_block"));
    }
    let tree = doc.get_tree(TREE);
    tree.enable_fractional_index(0);
    let mut ancestors = Vec::new();
    for (map, depth) in maps.iter().zip(depths) {
        ancestors.truncate(depth as usize);
        let node = tree
            .create(ancestors.last().copied())
            .map_err(|e| invalid(&e.to_string()))?;
        tree.get_meta(node)
            .map_err(|e| invalid(&e.to_string()))?
            .insert("block_cid", map.id().to_string())
            .map_err(|e| invalid(&e.to_string()))?;
        ancestors.push(node);
    }
    let meta = doc.get_map(META);
    meta.insert("version", 1).map_err(|e| invalid(&e.to_string()))?;
    meta.insert("initialized", true).map_err(|e| invalid(&e.to_string()))?;
    meta.insert("model", "rich_page_v4")
        .map_err(|e| invalid(&e.to_string()))?;
    Ok(true)
}
pub fn insert(doc: &LoroDoc, cid: &str, parent: Option<&str>, index: usize) -> Result<(), CollabError> {
    let tree = tree(doc)?;
    let placements = active_placements(&tree)?;
    let parent = parent
        .map(|cid| {
            placements
                .iter()
                .find(|entry| entry.cid == cid)
                .map(|entry| entry.tree_id)
                .ok_or_else(|| invalid("editor_structure_parent_not_found"))
        })
        .transpose()?;
    let node = tree.create_at(parent, index).map_err(|e| invalid(&e.to_string()))?;
    tree.get_meta(node)
        .map_err(|e| invalid(&e.to_string()))?
        .insert("block_cid", cid)
        .map_err(|e| invalid(&e.to_string()))?;
    Ok(())
}
pub fn move_block(doc: &LoroDoc, cid: &str, parent: Option<&str>, index: usize) -> Result<(), CollabError> {
    let placements = placements(doc)?.ok_or_else(|| invalid("editor_structure_required"))?;
    let node = placements
        .iter()
        .find(|entry| entry.cid == cid)
        .ok_or_else(|| invalid("editor_block_not_found"))?;
    let parent = parent
        .map(|cid| {
            placements
                .iter()
                .find(|entry| entry.cid == cid)
                .map(|entry| entry.tree_id)
                .ok_or_else(|| invalid("editor_structure_parent_not_found"))
        })
        .transpose()?;
    let parent_depth = parent
        .and_then(|id| {
            placements
                .iter()
                .find(|entry| entry.tree_id == id)
                .map(|entry| entry.depth + 1)
        })
        .unwrap_or(0);
    let height = placements
        .iter()
        .skip_while(|entry| entry.tree_id != node.tree_id)
        .skip(1)
        .take_while(|entry| entry.depth > node.depth)
        .map(|entry| entry.depth - node.depth)
        .max()
        .unwrap_or(0);
    if parent_depth + height > 7 {
        return Err(invalid("editor_structure_depth_limit"));
    }
    let mut ancestor = parent;
    while let Some(id) = ancestor {
        if id == node.tree_id {
            return Err(invalid("editor_structure_cycle"));
        }
        ancestor = match tree(doc)?.parent(id) {
            Some(TreeParentId::Node(parent)) => Some(parent),
            _ => None,
        };
    }
    tree(doc)?
        .mov_to(node.tree_id, parent, index)
        .map_err(|e| invalid(&e.to_string()))?;
    Ok(())
}
pub fn delete(doc: &LoroDoc, cid: &str) -> Result<(), CollabError> {
    let placements = placements(doc)?.ok_or_else(|| invalid("editor_structure_required"))?;
    let node = placements
        .iter()
        .find(|entry| entry.cid == cid)
        .ok_or_else(|| invalid("editor_block_not_found"))?;
    let mut remaining = 0;
    for placement in &placements {
        let mut ancestor = Some(placement.tree_id);
        let mut in_subtree = false;
        while let Some(id) = ancestor {
            if id == node.tree_id {
                in_subtree = true;
                break;
            }
            ancestor = match tree(doc)?.parent(id) {
                Some(TreeParentId::Node(parent)) => Some(parent),
                _ => None,
            };
        }
        if !in_subtree {
            remaining += 1;
        }
    }
    if remaining == 0 {
        return Err(invalid("editor_doc_requires_one_block"));
    }
    tree(doc)?.delete(node.tree_id).map_err(|e| invalid(&e.to_string()))?;
    Ok(())
}

/// Test-only count of full placement-tree walks (`placements`, `active_placements`) on this thread.
#[cfg(test)]
pub(crate) mod full_walks {
    use std::cell::Cell;
    thread_local! {
        static WALKS: Cell<usize> = const { Cell::new(0) };
    }
    pub fn record() {
        WALKS.with(|count| count.set(count.get() + 1));
    }
    pub fn reset() {
        WALKS.with(|count| count.set(0));
    }
    pub fn count() -> usize {
        WALKS.with(Cell::get)
    }
}

/// The tree handle of a [`PlacementIndex`]; handles compare equal so that index equality is about
/// placements only.
#[derive(Debug, Clone)]
struct TreeHandle(LoroTree);

impl PartialEq for TreeHandle {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for TreeHandle {}

/// One active placement as the session index keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexedNode {
    tree_id: TreeID,
    parent: Option<Arc<str>>,
    depth: u32,
}

/// The editor session's placement index: what `insert`, `move_block` and `delete` read of the
/// active placements, kept incrementally instead of being recomputed from the whole tree for
/// every operation.
///
/// It holds, for every active block CID, its tree node, its parent CID and its depth, and the set
/// of active children of every parent (and of the root level). It is built from [`placements`]
/// (so a document the full read refuses is refused exactly as before) and is updated by
/// [`insert_indexed`], [`move_indexed`] and [`delete_indexed`] with the effect each has on the
/// active tree. Child order is not kept: no operation reads it.
///
/// The index is valid only for the document state it was built or last updated at; the owner
/// (`LoroCollabEngine`) drops it whenever the document may have changed behind it, and after any
/// failed operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementIndex {
    /// The editor tree, read once by typed id when the index is built. A tree-root probe values
    /// the whole tree (O(page)), so the operations reuse this handle: nothing that can replace or
    /// shadow the root runs while the index is valid (any other change retires the index).
    tree: TreeHandle,
    nodes: HashMap<Arc<str>, IndexedNode>,
    roots: HashSet<Arc<str>>,
    children: HashMap<Arc<str>, HashSet<Arc<str>>>,
    /// Set when an update could not be applied to the index exactly; the index must then be
    /// rebuilt before use.
    stale: bool,
}

impl PlacementIndex {
    /// Builds the index with one full read of the placement tree.
    ///
    /// # Errors
    /// Whatever [`placements`] returns, and `editor_structure_required` when the structure
    /// metadata root does not say the placement index is initialised.
    pub fn build(doc: &LoroDoc) -> Result<Self, CollabError> {
        let entries = placements(doc)?.ok_or_else(|| invalid("editor_structure_required"))?;
        Ok(Self::from_placements(tree(doc)?, &entries))
    }

    /// The index of a full read's result, with the editor tree it was read from.
    #[must_use]
    pub fn from_placements(tree: LoroTree, entries: &[Placement]) -> Self {
        let mut index = Self {
            tree: TreeHandle(tree),
            nodes: HashMap::with_capacity(entries.len()),
            roots: HashSet::new(),
            children: HashMap::new(),
            stale: false,
        };
        for entry in entries {
            let cid: Arc<str> = Arc::from(entry.cid.as_str());
            let parent = entry.parent.as_deref().map(Arc::<str>::from);
            index.link(cid, entry.tree_id, parent, entry.depth);
        }
        index
    }

    /// Whether the index cannot be trusted any more and must be rebuilt.
    #[must_use]
    pub const fn is_stale(&self) -> bool {
        self.stale
    }

    /// Number of active placements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Whether `cid` has an active placement.
    #[must_use]
    pub fn contains(&self, cid: &str) -> bool {
        self.nodes.contains_key(cid)
    }

    fn link(&mut self, cid: Arc<str>, tree_id: TreeID, parent: Option<Arc<str>>, depth: u32) {
        match &parent {
            Some(parent) => self.children.entry(parent.clone()).or_default().insert(cid.clone()),
            None => self.roots.insert(cid.clone()),
        };
        self.nodes.insert(cid, IndexedNode { tree_id, parent, depth });
    }

    fn unlink_from_parent(&mut self, cid: &str, parent: Option<&Arc<str>>) {
        match parent {
            Some(parent) => {
                if let Some(siblings) = self.children.get_mut(parent.as_ref()) {
                    siblings.remove(cid);
                    if siblings.is_empty() {
                        self.children.remove(parent.as_ref());
                    }
                }
            }
            None => {
                self.roots.remove(cid);
            }
        }
    }

    fn child_count(&self, parent: Option<&str>) -> usize {
        parent.map_or(self.roots.len(), |parent| {
            self.children.get(parent).map_or(0, HashSet::len)
        })
    }

    /// `cid` and its active descendants, `cid` first.
    fn subtree(&self, cid: &str) -> Vec<Arc<str>> {
        let mut out = Vec::new();
        let Some((key, _)) = self.nodes.get_key_value(cid) else {
            return out;
        };
        let mut stack = vec![key.clone()];
        while let Some(next) = stack.pop() {
            if let Some(children) = self.children.get(next.as_ref()) {
                stack.extend(children.iter().cloned());
            }
            out.push(next);
        }
        out
    }

    /// The greatest depth below `cid` (0 for a leaf).
    fn height(&self, cid: &str) -> u32 {
        let base = self.nodes.get(cid).map_or(0, |node| node.depth);
        self.subtree(cid)
            .iter()
            .filter_map(|id| self.nodes.get(id.as_ref()))
            .map(|node| node.depth.saturating_sub(base))
            .max()
            .unwrap_or(0)
    }

    /// The active children count of `parent` (`None` for the root level) and the depth of
    /// `parent` when it is given; `None` when `parent` has no active placement.
    pub(crate) fn placement_bounds(&self, parent: Option<&str>) -> Option<(usize, Option<u32>)> {
        let Some(cid) = parent else {
            return Some((self.child_count(None), None));
        };
        self.nodes
            .get(cid)
            .map(|node| (self.child_count(Some(cid)), Some(node.depth)))
    }
}

/// [`insert`] against the session index: the same tree operations in the same order, with the
/// parent looked up in the index instead of in a fresh walk of the tree.
///
/// # Errors
/// As [`insert`]. The caller drops the index after an error.
pub fn insert_indexed(
    index: &mut PlacementIndex,
    cid: &str,
    parent: Option<&str>,
    position: usize,
) -> Result<(), CollabError> {
    let tree = index.tree.0.clone();
    let parent = parent
        .map(|cid| {
            index
                .nodes
                .get_key_value(cid)
                .map(|(key, node)| (key.clone(), node.tree_id, node.depth))
                .ok_or_else(|| invalid("editor_structure_parent_not_found"))
        })
        .transpose()?;
    let node = tree
        .create_at(parent.as_ref().map(|entry| entry.1), position)
        .map_err(|e| invalid(&e.to_string()))?;
    tree.get_meta(node)
        .map_err(|e| invalid(&e.to_string()))?
        .insert("block_cid", cid)
        .map_err(|e| invalid(&e.to_string()))?;
    if index.nodes.contains_key(cid) {
        // A second active node for one CID: a fresh read refuses that document, so the index
        // cannot describe it. Force a rebuild (which reports it as before).
        index.stale = true;
        return Ok(());
    }
    let depth = parent.as_ref().map_or(0, |entry| entry.2 + 1);
    index.link(Arc::from(cid), node, parent.map(|entry| entry.0), depth);
    Ok(())
}

/// [`move_block`] against the session index: the same checks in the same order and the same
/// `mov_to`, with the placement facts read from the index.
///
/// # Errors
/// As [`move_block`]. The caller drops the index after an error.
pub fn move_indexed(
    index: &mut PlacementIndex,
    cid: &str,
    parent: Option<&str>,
    position: usize,
) -> Result<(), CollabError> {
    let node = index
        .nodes
        .get(cid)
        .cloned()
        .ok_or_else(|| invalid("editor_block_not_found"))?;
    let parent = parent
        .map(|cid| {
            index
                .nodes
                .get_key_value(cid)
                .map(|(key, entry)| (key.clone(), entry.tree_id, entry.depth))
                .ok_or_else(|| invalid("editor_structure_parent_not_found"))
        })
        .transpose()?;
    let parent_depth = parent.as_ref().map_or(0, |entry| entry.2 + 1);
    if parent_depth + index.height(cid) > 7 {
        return Err(invalid("editor_structure_depth_limit"));
    }
    let tree = index.tree.0.clone();
    let mut ancestor = parent.as_ref().map(|entry| entry.0.clone());
    while let Some(id) = ancestor {
        if id.as_ref() == cid {
            return Err(invalid("editor_structure_cycle"));
        }
        ancestor = index.nodes.get(id.as_ref()).and_then(|entry| entry.parent.clone());
    }
    tree.mov_to(node.tree_id, parent.as_ref().map(|entry| entry.1), position)
        .map_err(|e| invalid(&e.to_string()))?;
    let new_parent = parent.map(|entry| entry.0);
    index.unlink_from_parent(cid, node.parent.as_ref());
    match &new_parent {
        Some(parent) => index.children.entry(parent.clone()).or_default().insert(Arc::from(cid)),
        None => index.roots.insert(Arc::from(cid)),
    };
    for id in index.subtree(cid) {
        let Some(entry) = index.nodes.get_mut(id.as_ref()) else {
            continue;
        };
        entry.depth = entry.depth - node.depth + parent_depth;
        if id.as_ref() == cid {
            entry.parent.clone_from(&new_parent);
        }
    }
    Ok(())
}

/// [`delete`] against the session index.
///
/// # Errors
/// As [`delete`]. The caller drops the index after an error.
pub fn delete_indexed(index: &mut PlacementIndex, cid: &str) -> Result<(), CollabError> {
    let node = index
        .nodes
        .get(cid)
        .cloned()
        .ok_or_else(|| invalid("editor_block_not_found"))?;
    let tree = index.tree.0.clone();
    let subtree = index.subtree(cid);
    if index.nodes.len() == subtree.len() {
        return Err(invalid("editor_doc_requires_one_block"));
    }
    tree.delete(node.tree_id).map_err(|e| invalid(&e.to_string()))?;
    index.unlink_from_parent(cid, node.parent.as_ref());
    for id in subtree {
        index.nodes.remove(id.as_ref());
        index.children.remove(id.as_ref());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CollabEngine, LoroCollabEngine, NodeKind, Operation};
    const SNAPSHOT: &[u8] = include_bytes!("../../../frontend/tests/fixtures/editor-doc-v1/browser.snapshot");
    #[test]
    fn historical_fork_retains_deleted_placement_metadata_and_ignores_later_ops() {
        let mut engine = LoroCollabEngine::new_empty(97);
        engine.initialize_page_editor().unwrap();
        engine.initialize_page_structure_model(5).unwrap();
        let placeholder = engine.semantic_snapshot().unwrap().nodes.keys().next().unwrap().clone();
        engine
            .apply_content_operation(&Operation::CreateNode {
                id: "survivor".into(),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            })
            .unwrap();
        engine
            .apply_content_operation(&Operation::DeleteNode { id: placeholder })
            .unwrap();
        let before = engine.semantic_snapshot().unwrap();
        let historical = engine.frontier();
        engine
            .apply_content_operation(&Operation::InsertText {
                id: "survivor".into(),
                index: 0,
                text: "future".into(),
            })
            .unwrap();
        let fork = engine.fork_at_frontier(&historical).unwrap();
        assert_eq!(fork.semantic_snapshot().unwrap(), before);
        assert_eq!(fork.frontier(), historical);
        let loaded = LoroCollabEngine::load(&fork.export_snapshot().unwrap()).unwrap();
        assert_eq!(loaded.semantic_snapshot().unwrap(), before);
        assert_eq!(loaded.frontier(), historical);
    }

    #[test]
    fn native_tree_move_nest_delete_create_preserve_content_cids_and_roundtrip() {
        let mut engine = LoroCollabEngine::load(SNAPSHOT).unwrap();
        let before = engine.semantic_snapshot().unwrap();
        assert!(!before.nodes.is_empty());
        assert!(engine.initialize_page_structure().unwrap());
        assert!(!engine.initialize_page_structure().unwrap());
        let indexed = engine.semantic_snapshot().unwrap();
        let roots: Vec<_> = indexed
            .nodes
            .iter()
            .filter(|(_, node)| node.parent.is_none())
            .map(|(id, _)| id.clone())
            .collect();
        let first = roots.first().unwrap().clone();
        let second = roots.get(1).unwrap().clone();
        engine
            .apply_content_operation(&Operation::MoveNode {
                id: first.clone(),
                new_parent: Some(second.clone()),
                index: 0,
            })
            .unwrap();
        let nested = engine.semantic_snapshot().unwrap();
        assert_eq!(nested.nodes.get(&first).unwrap().parent.as_ref(), Some(&second));
        engine
            .apply_content_operation(&Operation::InsertText {
                id: first.clone(),
                index: 0,
                text: "Moved ".into(),
            })
            .unwrap();
        engine
            .apply_content_operation(&Operation::CreateNode {
                id: "new-indexed".into(),
                parent: Some(second.clone()),
                index: 1,
                kind: NodeKind::Block,
            })
            .unwrap();
        engine
            .apply_content_operation(&Operation::InsertText {
                id: "new-indexed".into(),
                index: 0,
                text: "New".into(),
            })
            .unwrap();
        let created = engine.resolve_content_node_id(&"new-indexed".into()).unwrap();
        assert_eq!(
            engine
                .semantic_snapshot()
                .unwrap()
                .nodes
                .get(&created)
                .unwrap()
                .parent
                .as_ref(),
            Some(&second)
        );
        engine
            .apply_content_operation(&Operation::DeleteNode { id: first.clone() })
            .unwrap();
        let after = engine.semantic_snapshot().unwrap();
        assert!(!after.nodes.contains_key(&first));
        assert!(after.nodes.contains_key(&second));
        let loaded = LoroCollabEngine::load(&engine.export_snapshot().unwrap()).unwrap();
        assert_eq!(loaded.semantic_snapshot().unwrap(), after);
        assert_eq!(loaded.minimum_editor_model().unwrap(), 4);
        if std::env::var("WRITE_PAGE_V4_FIXTURE").as_deref() == Ok("1") {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/page-v4");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("rust-structured.snapshot"), engine.export_snapshot().unwrap()).unwrap();
            std::fs::write(
                root.join("rust-structured.semantic.json"),
                serde_json::to_vec_pretty(&after).unwrap(),
            )
            .unwrap();
        }
    }
    #[test]
    fn concurrent_uncoordinated_initializers_are_detected_not_silently_merged() {
        let source = LoroDoc::new();
        source.import(SNAPSHOT).unwrap();
        let a = source.fork();
        let b = source.fork();
        a.set_peer_id(9001).unwrap();
        b.set_peer_id(9002).unwrap();
        initialize(&a).unwrap();
        initialize(&b).unwrap();
        a.commit();
        b.commit();
        a.import(&b.export(loro::ExportMode::Snapshot).unwrap()).unwrap();
        assert!(placements(&a).is_err());
    }
}

#[cfg(test)]
mod js_interop_tests {
    use crate::{CollabEngine, LoroCollabEngine, Operation};
    #[test]
    fn actual_js_rich_tree_snapshot_accepts_rust_cid_edit_and_move() {
        let bytes = include_bytes!("../tests/fixtures/page-v4/js-rich.snapshot");
        let mut engine = LoroCollabEngine::load(bytes).unwrap();
        let before = engine.semantic_snapshot().unwrap();
        let unicode = before
            .nodes
            .iter()
            .find(|(_, node)| node.text == "o中文😀ne")
            .unwrap()
            .0
            .clone();
        let three = before
            .nodes
            .iter()
            .find(|(_, node)| node.text == "three")
            .unwrap()
            .0
            .clone();
        let two = before
            .nodes
            .iter()
            .find(|(_, node)| node.text == "two")
            .unwrap()
            .0
            .clone();
        assert_eq!(before.nodes.get(&two).unwrap().parent.as_ref(), Some(&unicode));
        let spans = before
            .nodes
            .get(&unicode)
            .unwrap()
            .properties
            .get("inline_spans")
            .unwrap()
            .clone();
        engine
            .apply_content_operation(&Operation::InsertText {
                id: unicode.clone(),
                index: u32::try_from(before.nodes.get(&unicode).unwrap().text.len()).unwrap(),
                text: " Rust".into(),
            })
            .unwrap();
        engine
            .apply_content_operation(&Operation::MoveNode {
                id: three.clone(),
                new_parent: None,
                index: 1,
            })
            .unwrap();
        let after = engine.semantic_snapshot().unwrap();
        assert_eq!(after.nodes.get(&unicode).unwrap().text, "o中文😀ne Rust");
        assert_eq!(
            after
                .nodes
                .get(&unicode)
                .unwrap()
                .properties
                .get("inline_spans")
                .unwrap(),
            &spans
        );
        let mut order: Vec<_> = after
            .nodes
            .iter()
            .filter(|(_, node)| node.properties.get("content_source").is_some_and(|v| v == "editor_doc"))
            .collect();
        order.sort_by_key(|(_, node)| &node.order_key);
        assert_eq!(
            order.iter().map(|(id, _)| (*id).clone()).collect::<Vec<_>>(),
            [unicode, two, three]
        );
        if std::env::var("WRITE_PAGE_V4_FIXTURE").as_deref() == Ok("1") {
            std::fs::write("/tmp/sv-page-rich-v4-rust.snapshot", engine.export_snapshot().unwrap()).unwrap();
        }
    }
}

#[cfg(test)]
mod native_undo_interop_tests {
    use crate::{CollabEngine, LoroCollabEngine};
    #[test]
    fn actual_js_native_tree_delete_undo_keeps_block_cid_with_new_placement_generation() {
        let mut engine =
            LoroCollabEngine::load(include_bytes!("../tests/fixtures/page-v4/js-tree-delete-undo.snapshot")).unwrap();
        let semantic = engine.semantic_snapshot().unwrap();
        let cid = std::sync::Arc::<str>::from("cid:21@991001:Map");
        assert_eq!(semantic.nodes.get(&cid).unwrap().text, "three");
        let parent = std::sync::Arc::<str>::from("cid:3@991001:Map");
        engine
            .apply_content_operation(&crate::Operation::MoveNode {
                id: cid.clone(),
                new_parent: Some(parent.clone()),
                index: 1,
            })
            .unwrap();
        assert_eq!(
            engine
                .semantic_snapshot()
                .unwrap()
                .nodes
                .get(&cid)
                .unwrap()
                .parent
                .as_ref(),
            Some(&parent)
        );
        engine
            .apply_content_operation(&crate::Operation::CreateNode {
                id: "restored-child".into(),
                parent: Some(cid.clone()),
                index: 0,
                kind: crate::NodeKind::Block,
            })
            .unwrap();
        engine
            .apply_content_operation(&crate::Operation::InsertText {
                id: "restored-child".into(),
                index: 0,
                text: "grandchild".into(),
            })
            .unwrap();
        engine
            .apply_content_operation(&crate::Operation::InsertText {
                id: cid.clone(),
                index: 0,
                text: "Restored ".into(),
            })
            .unwrap();
        let snapshot = engine.export_snapshot().unwrap();
        if std::env::var("WRITE_PAGE_V4_FIXTURE").as_deref() == Ok("1") {
            std::fs::write("/tmp/sv-page-tree-delete-undo-rust.snapshot", &snapshot).unwrap();
        }

        let loaded = LoroCollabEngine::load(&snapshot).unwrap();
        assert_eq!(
            loaded.semantic_snapshot().unwrap().nodes.get(&cid).unwrap().text,
            "Restored three"
        );
    }
}

#[cfg(test)]
mod creation_undo_tests {
    use crate::{CollabEngine, LoroCollabEngine};
    #[test]
    fn native_creation_undo_allows_known_storage_tombstone_but_no_live_dangling_reference() {
        let dangling = LoroCollabEngine::load(include_bytes!(
            "../tests/fixtures/page-v4/js-tree-create-redo-dangling.snapshot"
        ))
        .unwrap();
        assert!(dangling.semantic_snapshot().is_err());
        let engine =
            LoroCollabEngine::load(include_bytes!("../tests/fixtures/page-v4/js-tree-create-undo.snapshot")).unwrap();
        let semantic = engine.semantic_snapshot().unwrap();
        assert!(!semantic.nodes.values().any(|node| node.text == "new synthetic"));
        assert_eq!(
            semantic
                .nodes
                .values()
                .filter(|node| node
                    .properties
                    .get("content_source")
                    .is_some_and(|value| value == "editor_doc"))
                .count(),
            3
        );
    }
}
