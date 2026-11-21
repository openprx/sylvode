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
//! * [`CHARS_PER_ITEM`] characters of a text container (rounded up per container);
//! * [`BYTES_PER_ITEM`] bytes of the payload of the plain values of a map or list (rounded down per
//!   container): [`value_bytes`] counts eight bytes for every scalar, collection or container
//!   reference in a value, nested to any depth, plus the length of every string, binary and map key.
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
//! Payload (the same window, `window_calibration_tests::window_payload_calibration`, flat page of 400
//! blocks, values in the blocks' `attributes`; the cost per item is the full window over all items):
//!
//! | payload | items | full window | per item |
//! | --- | --- | --- | --- |
//! | 6 lists of 20,000 integers (about 1 MB) | 19,806 | 1.0 ms | 0.05 us |
//! | 16 strings of 60,000 characters | 19,808 | 0.7 ms | 0.03 us |
//! | 8 lists of 4,000 one-key maps | 13,304 | 2.3 ms | 0.17 us |
//! | 8 lists of 8,000 one-integer lists | 20,808 | 1.9 ms | 0.09 us |
//! | 8 maps of 3,000 keys | 9,544 | 3.0 ms | 0.32 us |
//!
//! Without the payload term these pages count 4,800 items whatever the payload: 40 writes of a list
//! of 20,000 integers (6.4 MB of payload), or 60 writes of a 60,000-character string, were all
//! decided incrementally, updates of 64 KB each. With it the
//! densest shape (the maps) costs 0.32 us per item, below the 0.58 us of the realistic pages, so
//! `BYTES_PER_ITEM` is 64: the budget's uncounted part, 16,000 items, then stands for about 1 MB
//! of payload, which is what `next`'s 50 ms window tolerated.
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
//! upper bound on the items the update added (the sum of [`op_work`] over its ops), compared with
//! the budget of the exact post-import totals.

use loro::{
    Container, JsonListOp, JsonMapOp, JsonMovableListOp, JsonOpContent, JsonTextOp, JsonTreeOp, LoroValue,
    ValueOrContainer,
};

/// Characters of one text container that count as one item.
pub const CHARS_PER_ITEM: u64 = 256;

/// Bytes of plain value payload (see [`value_bytes`]) that count as one item.
///
/// A map entry or a list element is one item whatever it holds, so a nested list, a map or a long
/// string stored as a plain value (a block's `attributes` value, a tree node's `prop:*` string) grew
/// a page without a work bound. The payload is charged on top, per container (rounded down, so the
/// small values of a realistic page cost nothing) and per op (rounded up, so the update's bound
/// stays an upper bound). The constant comes from `window_calibration_tests::window_payload_calibration`
/// (see the table in the module documentation).
pub const BYTES_PER_ITEM: u64 = 64;

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

/// The payload of a plain value, in bytes, recursively.
///
/// Eight for every scalar, a container reference or a collection, plus the length of a string or
/// binary and of every map key, plus the payload of every element. Iterative, so that a hostile
/// value nested tens of thousands deep cannot exhaust the stack.
#[must_use]
pub fn value_bytes(value: &LoroValue) -> u64 {
    let len = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
    let mut total: u64 = 0;
    let mut pending = vec![value];
    while let Some(next) = pending.pop() {
        total = total.saturating_add(8);
        match next {
            LoroValue::String(text) => total = total.saturating_add(len(text.len())),
            LoroValue::Binary(bytes) => total = total.saturating_add(len(bytes.len())),
            LoroValue::List(elements) => pending.extend(elements.iter()),
            LoroValue::Map(entries) => {
                for (key, entry) in entries.iter() {
                    total = total.saturating_add(len(key.len()));
                    pending.push(entry);
                }
            }
            LoroValue::Null
            | LoroValue::Bool(_)
            | LoroValue::Double(_)
            | LoroValue::I64(_)
            | LoroValue::Container(_) => {}
        }
    }
    total
}

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

/// An upper bound on what one op can add to a page.
///
/// It is its items and the bytes of plain value payload it carries (see [`value_bytes`]; a map key
/// counts too). Every item that exists after an import exists because an op of the base or of the update
/// created it, so for an update that cannot make the library materialise anything its ops do not
/// name (a strictly sequential one) the sum of this over its ops bounds the growth. Deletes add
/// nothing. An insert adds an entry or element and, when it inserts a container, the container;
/// a text insert adds the characters it carries (plus one for a container boundary). The caller
/// adds the items and `payload bytes / BYTES_PER_ITEM`, rounded up once for the whole update
/// ([`OpWork::items`]): rounding up per op would count a one-key map insert as several items.
/// The sum stays an upper bound: a container's count rounds its bytes down, and every op that
/// carries payload is counted at least one item more than it adds to the page (an entry or an
/// element is one item, an op counts two).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpWork {
    pub items: u64,
    pub payload_bytes: u64,
}

impl OpWork {
    /// The sum of two.
    #[must_use]
    pub const fn plus(self, other: Self) -> Self {
        Self {
            items: self.items.saturating_add(other.items),
            payload_bytes: self.payload_bytes.saturating_add(other.payload_bytes),
        }
    }

    /// The items this work adds to a page: its own and its payload's, the payload rounded up.
    #[must_use]
    pub const fn total_items(self) -> u64 {
        self.items.saturating_add(self.payload_bytes.div_ceil(BYTES_PER_ITEM))
    }
}

/// The work one op adds at most, see [`OpWork`].
#[must_use]
pub fn op_work(content: &JsonOpContent) -> OpWork {
    let len = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
    let bytes_of = |values: &mut dyn Iterator<Item = &LoroValue>| {
        values.fold(0_u64, |bytes, value| bytes.saturating_add(value_bytes(value)))
    };
    match content {
        JsonOpContent::List(JsonListOp::Insert { value, .. })
        | JsonOpContent::MovableList(JsonMovableListOp::Insert { value, .. }) => OpWork {
            items: len(value.len()).saturating_mul(2),
            payload_bytes: bytes_of(&mut value.iter()),
        },
        JsonOpContent::List(JsonListOp::Delete { .. })
        | JsonOpContent::MovableList(JsonMovableListOp::Delete { .. })
        | JsonOpContent::Map(JsonMapOp::Delete { .. })
        | JsonOpContent::Text(JsonTextOp::Delete { .. })
        | JsonOpContent::Tree(JsonTreeOp::Delete { .. }) => OpWork::default(),
        JsonOpContent::MovableList(JsonMovableListOp::Move { .. }) | JsonOpContent::Text(JsonTextOp::MarkEnd) => {
            OpWork {
                items: 1,
                payload_bytes: 0,
            }
        }
        JsonOpContent::Text(JsonTextOp::Mark { style_value, .. }) => OpWork {
            items: 1,
            payload_bytes: value_bytes(style_value),
        },
        JsonOpContent::MovableList(JsonMovableListOp::Set { value, .. }) => OpWork {
            items: 2,
            payload_bytes: value_bytes(value),
        },
        JsonOpContent::Map(JsonMapOp::Insert { key, value }) => OpWork {
            items: 2,
            payload_bytes: value_bytes(value).saturating_add(len(key.len())),
        },
        JsonOpContent::Tree(JsonTreeOp::Create { .. } | JsonTreeOp::Move { .. }) | JsonOpContent::Future(_) => OpWork {
            items: 2,
            payload_bytes: 0,
        },
        JsonOpContent::Text(JsonTextOp::Insert { text, .. }) => OpWork {
            items: len(text.chars().count()).div_ceil(CHARS_PER_ITEM).saturating_add(1),
            payload_bytes: 0,
        },
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

    /// One element of a list: a child container is counted, a plain value adds its payload bytes.
    fn add_element(&mut self, value: ValueOrContainer, depth: usize, within: &mut bool, bytes: &mut u64) {
        if !*within {
            return;
        }
        match value {
            ValueOrContainer::Container(child) => *within = self.add_container(&child, depth + 1),
            ValueOrContainer::Value(plain) => {
                *bytes = bytes.saturating_add(value_bytes(&plain));
                *within = self.add(0) && *bytes / BYTES_PER_ITEM <= self.ceiling;
            }
        }
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
                let mut bytes: u64 = 0;
                map.for_each(|key, value| {
                    if !within {
                        return;
                    }
                    bytes = bytes.saturating_add(u64::try_from(key.len()).unwrap_or(u64::MAX));
                    match value {
                        ValueOrContainer::Container(child) => within = self.add_container(&child, depth + 1),
                        ValueOrContainer::Value(plain) => {
                            bytes = bytes.saturating_add(value_bytes(&plain));
                            // Stop reading as soon as what is read alone is over the ceiling.
                            within = self.add(0) && bytes / BYTES_PER_ITEM <= self.ceiling;
                        }
                    }
                });
                within && self.add(bytes / BYTES_PER_ITEM)
            }
            Container::List(list) => {
                if !self.add(u64::try_from(list.len()).unwrap_or(u64::MAX)) {
                    return false;
                }
                let mut within = true;
                let mut bytes: u64 = 0;
                list.for_each(|value| self.add_element(value, depth, &mut within, &mut bytes));
                within && self.add(bytes / BYTES_PER_ITEM)
            }
            Container::MovableList(list) => {
                if !self.add(u64::try_from(list.len()).unwrap_or(u64::MAX)) {
                    return false;
                }
                let mut within = true;
                let mut bytes: u64 = 0;
                list.for_each(|value| self.add_element(value, depth, &mut within, &mut bytes));
                within && self.add(bytes / BYTES_PER_ITEM)
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_charged_by_its_payload_recursively() {
        assert_eq!(value_bytes(&LoroValue::Null), 8);
        assert_eq!(value_bytes(&LoroValue::from("abcd")), 12);
        let list: LoroValue = vec![LoroValue::from(1_i64), LoroValue::from(2_i64)].into();
        assert_eq!(value_bytes(&list), 8 + 8 + 8);
        let map: LoroValue = std::collections::HashMap::from([("key".to_owned(), list)]).into();
        assert_eq!(value_bytes(&map), 8 + 3 + 24);
    }

    #[test]
    fn a_value_nested_deeper_than_the_stack_allows_a_recursion_is_measured() {
        let mut value = LoroValue::from(1_i64);
        for _ in 0..2_000 {
            value = vec![value].into();
        }
        assert_eq!(value_bytes(&value), 8 * 2_001);
    }

    #[test]
    fn the_payload_of_an_update_is_rounded_up_once() {
        let one_key = |key: &str| {
            JsonOpContent::Map(JsonMapOp::Insert {
                key: key.to_owned(),
                value: LoroValue::from(1_i64),
            })
        };
        let work = op_work(&one_key("k"));
        assert_eq!(work.items, 2);
        assert_eq!(work.payload_bytes, 9);
        assert_eq!(work.total_items(), 3);
        let many = (0..64).fold(OpWork::default(), |sum, _| sum.plus(work));
        assert_eq!(many.items, 128);
        assert_eq!(many.payload_bytes, 576);
        assert_eq!(many.total_items(), 128 + 9);
    }
}
