//! The work measure of a page: how much the full projection of it reads, in one deterministic unit.
//!
//! The incremental window no longer pays for content that no documented limit counts (tree nodes
//! of kinds that count toward no total, nodes under deleted parents, keys of a block's
//! `attributes`, node `prop:*` keys, block maps no reader supports), so on its own it would let a
//! page grow through such content without bound, while the setup that computes the base facts and
//! the host's projection (both linear in the page) keep paying for it. On `next` the 50 ms window
//! was the accidental bound. This module restores a bound without changing a documented limit.
//!
//! # The unit
//!
//! One **item** is any of:
//!
//! * a tree node of the legacy tree, deleted or not, named or not (`LoroTree::nodes()`);
//! * an element of the root block list (`doc.children`);
//! * a container reached from one of those (the node's metadata map, a block map, an attribute map,
//!   an inline text, ...), counted once when it is entered;
//! * a map entry or a list element of such a container;
//! * an entry of the navigator `order` map or of the alias map;
//! * [`CHARS_PER_ITEM`] characters of a text container (rounded up per container).
//!
//! It is what the full projection reads: `legacy_package_semantic_snapshot_v1` visits every tree
//! node and its metadata; the block reader visits every element of the root list, its keys, its
//! `attributes` (deep) and its inline texts. It is proportional to the cost of the full window,
//! within a factor of about four across shapes. Measured on the development machine (release
//! build, `window_calibration_tests::window_calibration`), the full window (import, projection,
//! limits) of a base plus one trivial create:
//!
//! | page | items | full window | per item |
//! | --- | --- | --- | --- |
//! | flat, realistic, 9,950 blocks | 118,000 (11.9 per block) | 63 ms | 0.53 us |
//! | legacy, realistic, 10,000 nodes | 58,000 (5.8 per node) | 34 ms | 0.58 us |
//! | 4,000 `collection_field` nodes | 16,000 | 5.3 ms | 0.33 us |
//! | 16,000 `collection_field` nodes | 64,000 | 20.7 ms | 0.32 us |
//! | 8,000 navigators under a deleted parent | 32,000 | 12.4 ms | 0.39 us |
//! | 50,000 keys in one block's `attributes` | 55,000 | 13.0 ms | 0.24 us |
//! | 2,000 nodes with 100 `prop:*` keys each | 208,000 | 28 ms | 0.13 us |
//! | 8,000 unsupported block maps | 24,000 | 5.8 ms | 0.24 us |
//!
//! The reference runner is 3.6 times slower, and its CPU limit is 50 ms, so `next`'s window
//! accepts what costs up to 13.9 ms here: about 10,700 `collection_field` nodes, 53,000 attribute
//! keys or 19,000 unsupported maps. A page of 9,950 blocks costs 227 ms there, so `next` cannot
//! write to it at all, which is why the budget cannot be a bound on the whole page: what the
//! documented limits allow is let in, and only the rest is bounded.
//!
//! # The budget
//!
//! A page is within the budget when `items <= WORK_UNCOUNTED_ITEMS + ITEMS_PER_BLOCK * live
//! blocks + ITEMS_PER_NODE * live navigators + live characters / CHARS_PER_ITEM`.
//! `WORK_UNCOUNTED_ITEMS` is 16,000: at the worst cost per item of the table (0.58 us) the full
//! window at that many items takes 9.3 ms here and 33 ms on the reference runner, under its 50 ms
//! with a margin of one and a half; the shapes of the table with no live content stop at about 4,000
//! nodes, 14,000 attribute keys or 5,000 unsupported maps, a third to a quarter of where `next`
//! stops. The terms after
//! the first are what the documented limits already bound (10,000 live blocks, 10,000 live
//! navigators, 1,000,000 live characters): a page at the limits with realistic content stays
//! within the budget and stays on the incremental path. The first term is what nothing else
//! bounds. A page above the budget gets no base facts, so its windows are the full computation
//! (`next`'s behaviour: accepted while the window fits its CPU limit, refused by it beyond).
//!
//! The window keeps the measure without reading the page again: the items of the base plus an
//! upper bound on the items the update added (the sum of [`op_items`] over its ops), compared with
//! the budget of the exact post-import totals.

use loro::{
    Container, JsonListOp, JsonMapOp, JsonMovableListOp, JsonOpContent, JsonTextOp, JsonTreeOp, ValueOrContainer,
};

/// Characters of one text container that count as one item.
pub const CHARS_PER_ITEM: u64 = 256;

/// Items no documented limit bounds that a page may hold on top of what its live totals allow.
///
/// See the module documentation for the measurements this number comes from.
pub const WORK_UNCOUNTED_ITEMS: u64 = 16_000;

/// Items a live block of a realistic page uses, with margin: the block map, `nodeName`,
/// `attributes` and a few of its keys, `children` and a few inline texts.
pub const ITEMS_PER_BLOCK: u64 = 16;

/// Items a live navigator of a realistic legacy page uses, with margin: the node, its metadata map
/// and its `logical_id`, `kind`, `text` and a few property entries.
pub const ITEMS_PER_NODE: u64 = 10;

/// The deepest nesting of containers the measure follows. A page nested deeper is over the budget:
/// no reader of a realistic page nests that far, and a bounded depth keeps the traversal's stack
/// bounded for hostile input.
const MAX_DEPTH: usize = 32;

/// The budget for a page with these live totals.
#[must_use]
pub const fn item_budget(blocks: u64, navigators: u64, chars: u64) -> u64 {
    WORK_UNCOUNTED_ITEMS
        .saturating_add(ITEMS_PER_BLOCK.saturating_mul(blocks))
        .saturating_add(ITEMS_PER_NODE.saturating_mul(navigators))
        .saturating_add(chars / CHARS_PER_ITEM)
}

/// The largest budget any page can have: the one of a page at every documented limit.
///
/// A page with more items than this is over the budget whatever its totals, which lets the
/// traversal stop there before the projection that computes the totals runs.
#[must_use]
pub const fn item_budget_ceiling() -> u64 {
    use crate::limits::{CONTAINER_COUNT_MAX, DOCUMENT_BLOCK_COUNT_MAX, DOCUMENT_TEXT_CHARS_MAX};
    item_budget(
        DOCUMENT_BLOCK_COUNT_MAX as u64,
        CONTAINER_COUNT_MAX as u64,
        DOCUMENT_TEXT_CHARS_MAX as u64,
    )
}

/// An upper bound on the items one op can add to a page.
///
/// Every item that exists after an import exists because an op of the base or of the update
/// created it, so for an update that cannot make the library materialise anything its ops do not
/// name (a strictly sequential one) the sum of this over its ops bounds the growth. Deletes add
/// nothing. An insert adds an entry or element and, when it inserts a container, the container;
/// a text insert adds the characters it carries (plus one for a container boundary).
#[must_use]
pub fn op_items(content: &JsonOpContent) -> u64 {
    let len = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
    match content {
        JsonOpContent::List(JsonListOp::Insert { value, .. })
        | JsonOpContent::MovableList(JsonMovableListOp::Insert { value, .. }) => len(value.len()).saturating_mul(2),
        JsonOpContent::List(JsonListOp::Delete { .. })
        | JsonOpContent::MovableList(JsonMovableListOp::Delete { .. })
        | JsonOpContent::Map(JsonMapOp::Delete { .. })
        | JsonOpContent::Text(JsonTextOp::Delete { .. })
        | JsonOpContent::Tree(JsonTreeOp::Delete { .. }) => 0,
        JsonOpContent::MovableList(JsonMovableListOp::Move { .. })
        | JsonOpContent::Text(JsonTextOp::Mark { .. } | JsonTextOp::MarkEnd) => 1,
        JsonOpContent::MovableList(JsonMovableListOp::Set { .. })
        | JsonOpContent::Map(JsonMapOp::Insert { .. })
        | JsonOpContent::Tree(JsonTreeOp::Create { .. } | JsonTreeOp::Move { .. })
        | JsonOpContent::Future(_) => 2,
        JsonOpContent::Text(JsonTextOp::Insert { text, .. }) => {
            len(text.chars().count()).div_ceil(CHARS_PER_ITEM).saturating_add(1)
        }
    }
}

/// Counts items against a ceiling and stops counting once it is passed.
#[derive(Debug)]
pub(crate) struct ItemCount {
    total: u64,
    ceiling: u64,
}

impl ItemCount {
    pub(crate) const fn new(ceiling: u64) -> Self {
        Self { total: 0, ceiling }
    }

    /// Adds `n` items. False once the ceiling is passed.
    pub(crate) const fn add(&mut self, n: u64) -> bool {
        self.total = self.total.saturating_add(n);
        self.total <= self.ceiling
    }

    pub(crate) const fn total(&self) -> u64 {
        self.total
    }

    /// Adds the items of a container and everything below it: false once the ceiling is passed or
    /// the nesting is deeper than [`MAX_DEPTH`].
    pub(crate) fn add_container(&mut self, container: &Container, depth: usize) -> bool {
        if depth > MAX_DEPTH || !self.add(1) {
            return false;
        }
        match container {
            Container::Map(map) => {
                // The size is known before anything is read, so a huge map costs O(1) here.
                if !self.add(u64::try_from(map.len()).unwrap_or(u64::MAX)) {
                    return false;
                }
                let mut within = true;
                map.for_each(|_, value| {
                    if within && let ValueOrContainer::Container(child) = value {
                        within = self.add_container(&child, depth + 1);
                    }
                });
                within
            }
            Container::List(list) => {
                if !self.add(u64::try_from(list.len()).unwrap_or(u64::MAX)) {
                    return false;
                }
                let mut within = true;
                list.for_each(|value| {
                    if within && let ValueOrContainer::Container(child) = value {
                        within = self.add_container(&child, depth + 1);
                    }
                });
                within
            }
            Container::MovableList(list) => {
                if !self.add(u64::try_from(list.len()).unwrap_or(u64::MAX)) {
                    return false;
                }
                let mut within = true;
                list.for_each(|value| {
                    if within && let ValueOrContainer::Container(child) = value {
                        within = self.add_container(&child, depth + 1);
                    }
                });
                within
            }
            Container::Text(text) => self.add(
                u64::try_from(text.len_unicode())
                    .unwrap_or(u64::MAX)
                    .div_ceil(CHARS_PER_ITEM),
            ),
            // A tree nested in a document is not read by the projection; its nodes are counted so
            // that it cannot hide growth.
            Container::Tree(tree) => self.add(u64::try_from(tree.nodes().len()).unwrap_or(u64::MAX)),
            _ => true,
        }
    }
}
