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
//! * an entry of the navigator `order` map or of the alias map, with its key and the payload of its
//!   value charged like those of any other map (the server bounds the keys it writes itself, a client
//!   update does not);
//! * [`CHARS_PER_ITEM`] characters of a text container (rounded up per container);
//!   The payload of its style marks and its internal pieces are counted on top, per delta segment and
//!   per piece (see "Text mark values");
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
//! blocks + ITEMS_PER_NODE * live navigators + live characters / CHARS_PER_ITEM + formatting`, where
//! `formatting` is the page's mark items (see "Text mark values") up to [`MARK_ITEMS_PER_BLOCK`] per
//! live block ([`item_budget_with_marks`]; see "The formatting allowance").
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
//! # Text mark values
//!
//! The reader converts every attribute of every delta segment of a text (`page_inline::spans`), so
//! a large mark value costs once per segment, and an update that splits a text adds segments that
//! carry the values of marks it does not mention. The inline texts of the blocks the block reader
//! converts (supported kind, not a dependency node: [`block_mark_items`]) therefore count, on top of
//! their characters ([`text_mark_items`]):
//!
//! * the key and [`value_bytes`] of every attribute of every delta segment, at least
//!   [`ATTRIBUTE_MIN_BYTES`] for an attribute the reader skips (`null`, `false`) and at least
//!   [`LIVE_ATTRIBUTE_MIN_BYTES`] for one it keeps (it builds a span, sorts and serialises it);
//! * [`SEGMENT_BYTES`] for every segment after the first;
//! * for every internal piece of a segment after the first piece of that segment, [`PIECE_BYTES`]
//!   and the attributes of the segment once more (a key and [`value_bytes`], at least
//!   [`PIECE_ATTRIBUTE_MIN_BYTES`] each).
//!
//! The last term is what the library's `to_delta` costs: it builds and compares an attribute map for
//! every internal piece of the text before it merges equal neighbours, so the cost is the pieces
//! times the attributes, whatever the merged segments look like. Text typed in reverse order, or
//! interleaved by two peers, keeps one piece per character through a snapshot. The pieces are
//! counted with `LoroText::iter` before the delta is built. The copy `iter` makes is linear in the
//! text's characters (which the measure and the documented limit bound), but the call also runs the
//! library's lazy decode of the text's state, which costs its style anchors and annotations and is
//! not priced here. A text whose pieces alone pass the ceiling is refused without `to_delta` (the
//! decode has run by then, and the early exit can only fire when the ceiling left is below about
//! 28,000 items, which on a page of more than about 700 blocks it never is). The decode is what
//! [`crate::setup_guard`] bounds in the setup.
//!
//! The sum is divided by [`BYTES_PER_ITEM`] and rounded down per text, with an early exit at the
//! ceiling. No value shape is refused. Every other text (a value of the `order` or alias map, a text
//! under another key of a block, a text of a block the reader skips) is never converted by the
//! projection, and counts by its characters only: converting it would be work that no window ever
//! bounded, in a setup that has no limit.
//!
//! Calibration (`window_calibration_tests::window_dense_mark_calibration`, release, flat page of 50
//! blocks, the shape in the first block, each grown until the page is at its budget, so the rows are
//! comparable; the incremental window converts the touched text twice, once for the re-count and once
//! for the evaluation, and is bounded by the 13.9 ms the reference runner's 50 ms is here):
//!
//! | shape at the budget | items | full window | per item | incremental, dense block |
//! | --- | --- | --- | --- | --- |
//! | `bold` on every 8th character, 15 `false` keys over all | 18,829 | 5.9 ms | 0.31 us | 8.0 ms |
//! | `bold` on alternate characters | 18,838 | 4.4 ms | 0.23 us | 4.8 ms |
//! | all seven marks on every second segment | 18,805 | 7.0 ms | 0.37 us | 5.0 ms |
//! | typed in reverse, no marks (64,643 pieces) | 19,052 | 4.7 ms | 0.25 us | 8.9 ms |
//! | typed in reverse, 15 full-range `false` marks (4,038 pieces) | 18,811 | 3.9 ms | 0.21 us | 11.1 ms |
//!
//! Before the live-attribute and piece terms the seven-mark shape cost 1.34 us per item and the
//! reverse-typed shape with marks was charged as one segment. A write to another block costs 0.03 to
//! 0.06 ms in the mark shapes and 2.7 ms where the dense text has tens of thousands of pieces (the
//! import of the update into the page).
//!
//! The review's page (`window_fragmented_calibration`: 240 blocks, each 4,000 reverse-typed
//! characters, about 15 full-range marks) costs 1.0 s to project here (3.7 s on the reference
//! runner) and its setup converted every block three times (2.6 s here, 9.2 s there, over the 5 s
//! setup timeout). It is now 4.3 million items against a budget of about 33,000, refused after 165 ms of
//! setup (0.6 s on the reference runner). The largest fragmented pages within the budget (4,000
//! characters per block, `facts` present): 23 such blocks without marks on a page of 240 blocks (setup
//! 22 ms, full projection 9 ms), 24 blocks with 15 marks on a page of 9,950 blocks (setup 379 ms,
//! 1.4 s on the reference runner; projection 142 ms). The setup converts a text's delta before it
//! knows what the delta costs, so a stored page that was never accepted under this measure can
//! cost more to count than it is charged: pieces times marks, linear in the pieces of one text
//! (at most the 1,000,000 characters of the limit) and in its marks. A page written through the
//! incremental window since the pieces are charged was within the measure when it was accepted;
//! a page accepted by the full window (a fallback, a concurrent import, `incremental_window =
//! false`) or before this measure was charged by nothing of the kind. Each of its texts was
//! converted inside a window that metered it, though, and the count stops at the ceiling, so the
//! setup pays at most the ceiling's worth of charged work plus one text. What the measure cannot
//! see at all (style anchors, null-valued styles) is bounded by the setup guard
//! ([`crate::setup_guard`]), not by this measure.
//!
//! An op cannot state the segments it adds, so the window does not bound this part by the ops. The
//! facts keep the mark items of every block that has any, and the window re-counts the blocks the
//! update touched, on the document after the import and before any unit is evaluated; the growth
//! of the touched blocks is added to the update's bound. The walk of a block stops once its items
//! pass its base figure plus the headroom the budget has left, so the payload it reads is bounded by
//! what the touched blocks already cost plus the headroom, whatever the update holds. A block that
//! the reader did not read in the base (an unsupported `nodeName`, a dependency attribute) has no
//! figure, and is re-counted when the update touches it, which is how it can become readable. Legacy
//! tree nodes are not counted: the projection reads their text as a plain string (`to_string`), so
//! their marks cost it nothing.
//!
//! # The formatting allowance
//!
//! Honest formatting is dense: a bold, an italic and a link that overlap cut a text into five
//! segments and cost 22 items, six marks cost 40. A budget of [`ITEMS_PER_BLOCK`] per block is
//! calibrated on the unformatted realistic page, so heavy formatting on a few thousand blocks
//! crossed the budget, and the full window refuses a page of more than about 1,000 realistic blocks
//! outright. The mark items of a page therefore also count against the allowance of
//! [`MARK_ITEMS_PER_BLOCK`] (40) per live block, up to what the page holds: `budget + min(mark
//! items, 40 * blocks)`. Nothing else gains from it (maps, attribute keys and tree nodes are bounded
//! as before), and formatting beyond the allowance counts in full, so the shapes that grow a page
//! through marks (large values over many segments, many keys, many pieces) still reach the budget:
//! their shared part is 16,000 items plus 40 per block.
//!
//! Honest pages, 80-character blocks (`window_formatting_calibration`, release; items per block
//! include the 11.9 of the unformatted page; "ref" is x3.6):
//!
//! | page | items per block | budget per block | setup (ref) | full projection (ref) |
//! | --- | --- | --- | --- | --- |
//! | 1,200 blocks, two bold+italic+link clusters | 56.4 (was 28.4) | 61.6 (was 29.6) | 36 ms (129 ms) | 16 ms (59 ms) |
//! | 2,000 blocks, six marks | 52.4 (was 27.4) | 56.3 (was 24.3) | 62 ms (223 ms) | 27 ms (97 ms) |
//! | 9,950 blocks, one cluster | 34.4 (was 20.4) | 39.9 (was 17.9) | 221 ms (797 ms) | 105 ms (376 ms) |
//! | 9,950 blocks, two clusters | 56.4 | 57.9 | 320 ms (1.15 s) | 150 ms (541 ms) |
//!
//! All of them stay incremental. Before the allowance, the 2,000-block six-mark page and the
//! 9,950-block cluster page were over the budget (27.4 > 24.3, 20.4 > 17.9 items per block) and took
//! the full window, which the reference runner's limit refuses for pages of that size. The budget
//! of a page at the block limit with the allowance is about 590,000 items, which costs the setup
//! about 0.6 s here (2.2 s on the reference runner), under its 5 s timeout, at the 0.2 to 0.4 us per
//! item the calibrated formatting shapes cost.
//!
//! Edited pages (`window_setup_guard_tests::window_edit_history_calibration`, release): blocks that
//! carry the pieces of 30 single-character edits by two peers lose the incremental path sooner. Two
//! clusters per block: 72.5 items per block against a budget of 79.2 at 700 blocks (incremental),
//! 72.4 against 72.3 at 1,000 blocks and 72.7 against 69.6 at 1,200 (full window); six marks on
//! 2,000 blocks: 71.3 against 64.3; one cluster on 3,000 blocks: 46.9 against 56.1 (incremental);
//! two clusters on 1,200 blocks with 10 edits per block: 63.2 against 69.6 (incremental). The
//! setup of these pages costs 76 to 240 MiB of allocation, far under the setup guard's limit, so
//! the allowance is what decides them. No behaviour changes: the figures are the measurement the
//! review asked for.
//!
//! The window keeps the measure without reading the page again: the items of the base plus an
//! upper bound on the items the update added (the sum of [`op_work`] over its ops), compared with
//! the budget of the exact post-import totals.

use std::collections::HashMap;

use loro::{
    Container, JsonListOp, JsonMapOp, JsonMovableListOp, JsonOpContent, JsonTextOp, JsonTreeOp, LoroMap, LoroText,
    LoroValue, TextDelta, ValueOrContainer,
};

use crate::operation::NodeId;
use crate::setup_guard::{Fired, SetupGuard, TextMark};

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

/// Bytes the measure charges for every attribute instance of a text segment, at least.
///
/// The key and the value of a mark can be a few bytes (`false` under a one-letter key is 9), but
/// converting one instance costs an allocation, a hash insert and a conversion, about 0.2 us, which
/// is a third of an item. See `window_calibration_tests::window_dense_mark_calibration`.
pub const ATTRIBUTE_MIN_BYTES: u64 = 24;

/// Bytes the measure charges for every attribute instance the reader keeps, at least.
///
/// The reader keeps an instance whose value is neither `null` nor `false`: it builds a span for it,
/// sorts the spans of the block and serialises them (`page_inline::spans`, `prose_projection`), about 0.8 us, against 0.2 us for an
/// instance it skips. See `window_calibration_tests::window_dense_mark_calibration`.
pub const LIVE_ATTRIBUTE_MIN_BYTES: u64 = 128;

/// Bytes the measure charges for every segment of a text after the first.
///
/// A text with marks is cut into segments, each of which the reader walks and builds spans for,
/// about 0.4 us, which is three quarters of an item. Charged only to a text that is cut at all, so
/// the unmarked text of a realistic page pays nothing.
pub const SEGMENT_BYTES: u64 = 48;

/// Bytes the measure charges for every internal piece of a segment after the first.
///
/// See [`text_mark_items`]. The library builds and compares an attribute map for every
/// piece, whether or not it carries any attribute.
pub const PIECE_BYTES: u64 = 18;

/// Bytes the measure charges for every attribute instance of a piece after the first, at least.
///
/// The library copies the key and the value of every
/// attribute for every piece. See `window_calibration_tests::window_fragmented_calibration`.
pub const PIECE_ATTRIBUTE_MIN_BYTES: u64 = 18;

/// Items no documented limit bounds that a page may hold on top of what its live totals allow.
///
/// See the module documentation for the measurements this number comes from.
pub const WORK_UNCOUNTED_ITEMS: u64 = 16_000;

/// Items a live block of a realistic page uses, with margin: the block map, `nodeName`,
/// `attributes` and a few of its keys, `children` and a few inline texts.
pub const ITEMS_PER_BLOCK: u64 = 16;

/// Items of text formatting that a live block may hold without them counting against the budget.
///
/// Formatting is marks and internal pieces, see [`text_mark_items`]. It counts against the allowance
/// as long as the page holds that many in all: a
/// few overlapping clusters of bold, italic and a link, or six marks, per block, which is what an
/// honest, heavily formatted document looks like. See the module documentation ("The formatting
/// allowance") for the measurements. Formatting beyond the allowance counts in full, and all the
/// other content (maps, attribute keys, tree nodes) is bounded as before.
pub const MARK_ITEMS_PER_BLOCK: u64 = 40;

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

/// The budget of a page with these live totals that holds `mark_items` items of text formatting
/// (see [`MARK_ITEMS_PER_BLOCK`]): [`item_budget`] plus the formatting that fits the allowance.
#[must_use]
pub const fn item_budget_with_marks(blocks: u64, navigators: u64, chars: u64, mark_items: u64) -> u64 {
    let allowance = MARK_ITEMS_PER_BLOCK.saturating_mul(blocks);
    item_budget(blocks, navigators, chars).saturating_add(if mark_items < allowance { mark_items } else { allowance })
}

/// The largest budget any page can have: the one of a page at every documented limit, with
/// formatting up to the allowance.
///
/// A page with more items than this is over the budget whatever its totals, which lets the
/// traversal stop there before the projection that computes the totals runs.
#[must_use]
pub const fn item_budget_ceiling() -> u64 {
    use crate::limits::{CONTAINER_COUNT_MAX, DOCUMENT_BLOCK_COUNT_MAX, DOCUMENT_TEXT_CHARS_MAX};
    item_budget_with_marks(
        DOCUMENT_BLOCK_COUNT_MAX as u64,
        CONTAINER_COUNT_MAX as u64,
        DOCUMENT_TEXT_CHARS_MAX as u64,
        u64::MAX,
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

/// The items of the style marks and of the internal pieces of one text, in the unit of
/// [`BYTES_PER_ITEM`] bytes, rounded down:
///
/// * the payload of every attribute of every delta segment (a key and [`value_bytes`] of its value,
///   at least [`ATTRIBUTE_MIN_BYTES`]), and [`SEGMENT_BYTES`] for every segment after the first;
/// * for every internal piece of a segment after the first piece of that segment, [`PIECE_BYTES`] and
///   the attributes of the segment again (a key and [`value_bytes`], at least
///   [`PIECE_ATTRIBUTE_MIN_BYTES`] each).
///
/// This is what the reader pays: `page_inline::spans` converts every attribute of every segment, so a
/// large mark value costs once per segment, and every mark that splits a text adds segments that
/// carry the values of the marks around it. Below that, the library's `to_delta` builds the
/// attribute map of every internal piece of the text and compares it with the previous one before it
/// merges equal neighbours, so the conversion costs the pieces times the attributes of their
/// segment, whatever the merged output looks like. Text typed in reverse order, or interleaved by
/// two peers, keeps one piece per character through a snapshot.
///
/// The pieces are counted first (`LoroText::iter`: the copy it makes is linear in the characters,
/// which the measure and the documented limit bound, but the call also runs the library's lazy
/// decode of the text's state, which the measure does not price), and the count stops the work as
/// soon as the pieces alone pass `stop_over`, before `to_delta` is called. The delta itself cannot
/// stop early: its cost is the pieces times the attributes, which the base figure counted (setup) or
/// which the metered import paid for (window). `None` as soon as the items counted so far pass
/// `stop_over`.
pub(crate) fn text_mark_items(text: &LoroText, stop_over: u64, guard: Option<&SetupGuard>) -> Result<u64, Stop> {
    #[cfg(test)]
    crate::walk_counter::record_delta_read(loro::ContainerTrait::id(text).to_string());
    // The text's own cost, measured from here: its decode (the `iter` below) and its conversion.
    let mark = guard.map(SetupGuard::text_start);
    #[cfg(test)]
    if crate::window_check::fault::active() == Some(crate::window_check::fault::Fault::PieceCountSkipped) {
        return text_segment_items(text, &[], stop_over, guard.zip(mark));
    }
    let mut pieces: Vec<usize> = Vec::new();
    let mut within = true;
    text.iter(|chunk| {
        pieces.push(chunk.chars().count());
        // Stop as soon as the pieces alone are over the ceiling.
        within = (pieces.len() as u64).saturating_mul(PIECE_BYTES) / BYTES_PER_ITEM <= stop_over;
        within
    });
    if !within {
        return Err(Stop::Ceiling);
    }
    text_segment_items(text, &pieces, stop_over, guard.zip(mark))
}

/// The segment and piece charges of [`text_mark_items`], given the lengths in characters of the
/// text's internal pieces in order.
fn text_segment_items(
    text: &LoroText,
    pieces: &[usize],
    stop_over: u64,
    guard: Option<(&SetupGuard, TextMark)>,
) -> Result<u64, Stop> {
    // The decode of the text's style state (the `iter` before) is paid for; the conversion of its
    // delta, which a hostile shape makes the dearer part, is not started when the setup is over or
    // when the decode alone has cost this text more than its budget.
    if let Some((guard, mark)) = guard
        && let Some(fired) = guard.fired().or_else(|| guard.text_fired(mark))
    {
        return Err(Stop::Guard(fired));
    }
    let mut bytes: u64 = 0;
    let mut segments: u64 = 0;
    // The piece that the next segment starts with, and where that piece starts.
    let mut next_piece = 0_usize;
    let mut piece_start = 0_usize;
    let mut segment_start = 0_usize;
    for delta in text.to_delta() {
        let TextDelta::Insert { insert, attributes } = delta else {
            continue;
        };
        // A text with no marks is one segment, which costs what its characters cost.
        if segments > 0 {
            bytes = bytes.saturating_add(SEGMENT_BYTES);
        }
        segments += 1;
        let mut attribute_bytes: u64 = 0;
        let mut piece_attribute_bytes: u64 = 0;
        for (key, value) in attributes.iter().flatten() {
            let payload = u64::try_from(key.len())
                .unwrap_or(u64::MAX)
                .saturating_add(value_bytes(value));
            let kept = !matches!(value, LoroValue::Null | LoroValue::Bool(false));
            let floor = if kept {
                LIVE_ATTRIBUTE_MIN_BYTES
            } else {
                ATTRIBUTE_MIN_BYTES
            };
            attribute_bytes = attribute_bytes.saturating_add(payload.max(floor));
            piece_attribute_bytes = piece_attribute_bytes.saturating_add(payload.max(PIECE_ATTRIBUTE_MIN_BYTES));
        }
        bytes = bytes.saturating_add(attribute_bytes);
        // The pieces that start inside this segment, all but the first.
        let segment_end = segment_start.saturating_add(insert.chars().count());
        let mut in_segment: u64 = 0;
        while let Some(length) = pieces.get(next_piece)
            && piece_start < segment_end
        {
            in_segment += 1;
            piece_start = piece_start.saturating_add(*length);
            next_piece += 1;
        }
        segment_start = segment_end;
        bytes = bytes.saturating_add(
            in_segment
                .saturating_sub(1)
                .saturating_mul(PIECE_BYTES.saturating_add(piece_attribute_bytes)),
        );
        if bytes / BYTES_PER_ITEM > stop_over {
            return Err(Stop::Ceiling);
        }
    }
    // The conversion is paid for too: a text whose decode and conversion together passed its budget
    // yields no facts, whichever of the two was the dear part.
    if let Some((guard, mark)) = guard
        && let Some(fired) = guard.text_fired(mark)
    {
        return Err(Stop::Guard(fired));
    }
    Ok(bytes / BYTES_PER_ITEM)
}

/// The mark items (see [`text_mark_items`]) of the texts of one editor block that the block reader
/// converts: the direct text elements of its `children` list, and only when the reader reads the
/// block at all ([`crate::editor_doc_bridge::reads_inline_marks`]). Every other text of a page (a
/// text stored as a value of the `order` or alias map, under another key of a block or in its
/// `attributes`, or in a block the reader skips) is never converted by the projection and counts by
/// its characters only; converting it here would be work no window ever bounded.
///
/// Rounded down per text, so the sum equals what the base count adds. `None` as soon as the items
/// counted so far pass `stop_over`.
pub(crate) fn block_mark_items(block: &LoroMap, stop_over: u64) -> Option<u64> {
    block_mark_items_guarded(block, stop_over, None).ok()
}

/// [`block_mark_items`] for the setup: the text-by-text conversions also stop once the setup's
/// `guard` has passed its limit ([`Stop::Guard`]).
pub(crate) fn block_mark_items_guarded(
    block: &LoroMap,
    stop_over: u64,
    guard: Option<&SetupGuard>,
) -> Result<u64, Stop> {
    if !crate::editor_doc_bridge::reads_inline_marks(block) {
        return Ok(0);
    }
    let Some(ValueOrContainer::Container(Container::List(children))) = block.get("children") else {
        return Ok(0);
    };
    let mut items: u64 = 0;
    for index in 0..children.len() {
        if let Some(ValueOrContainer::Container(Container::Text(text))) = children.get(index) {
            items = items.saturating_add(text_mark_items(&text, stop_over.saturating_sub(items), guard)?);
            if items > stop_over {
                return Err(Stop::Ceiling);
            }
            if let Some(fired) = guard.and_then(SetupGuard::fired) {
                return Err(Stop::Guard(fired));
            }
        }
    }
    Ok(items)
}

/// Why a count of mark items stopped before it was done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    /// The items passed the ceiling the caller gave.
    Ceiling,
    /// A setup guard fired ([`crate::setup_guard`]).
    Guard(Fired),
}

/// Counts items against a ceiling and stops counting once it is passed.
#[derive(Debug)]
pub(crate) struct ItemCount {
    total: u64,
    ceiling: u64,
    guard: Option<SetupGuard>,
    stopped_by_guard: Option<Fired>,
    /// The mark items of every editor block that has any, by the block's key: what the base facts
    /// keep, so that the setup converts a marked text once for the count and not again for them.
    marks: HashMap<NodeId, u64>,
}

impl ItemCount {
    pub(crate) fn new(ceiling: u64) -> Self {
        Self {
            total: 0,
            ceiling,
            guard: None,
            stopped_by_guard: None,
            marks: HashMap::new(),
        }
    }

    /// A count of the setup: it stops, with [`ItemCount::stopped_by_guard`] set, once `guard` has
    /// passed its limit.
    pub(crate) fn guarded(ceiling: u64, guard: SetupGuard) -> Self {
        Self {
            guard: Some(guard),
            ..Self::new(ceiling)
        }
    }

    /// Adds the mark items of an editor block (see [`block_mark_items`]). False once the ceiling is
    /// passed or the setup guard fired.
    pub(crate) fn add_block_marks(&mut self, block: &LoroMap) -> bool {
        match block_mark_items_guarded(block, self.ceiling.saturating_sub(self.total), self.guard.as_ref()) {
            Ok(marks) => {
                if marks > 0 {
                    self.marks
                        .insert(NodeId::from(loro::ContainerTrait::id(block).to_string()), marks);
                }
                self.add(marks)
            }
            Err(Stop::Ceiling) => false,
            Err(Stop::Guard(fired)) => {
                self.stopped_by_guard = Some(fired);
                false
            }
        }
    }

    /// Which setup guard stopped the count, if one did and not the ceiling.
    pub(crate) const fn stopped_by_guard(&self) -> Option<Fired> {
        self.stopped_by_guard
    }

    /// The per-block mark items the count saw (complete only for a count that finished).
    pub(crate) fn into_marks(self) -> HashMap<NodeId, u64> {
        self.marks
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

    /// `LoroCollabEngine::mark_items_added` skips a block with no mark items when the update carries
    /// no mark op. That is sound because an insert can then add at most two pieces to a text whose
    /// pieces cost nothing yet (one segment with at most two attributes the reader skips): the
    /// charge of those pieces must stay within what the op is charged (two items at least, for
    /// [`BYTES_PER_ITEM`] bytes each). It breaks when a piece constant grows past about 28 bytes.
    #[test]
    fn the_pieces_an_insert_can_add_to_an_unmarked_text_cost_less_than_the_insert_is_charged() {
        let added = 2 * (PIECE_BYTES + 2 * PIECE_ATTRIBUTE_MIN_BYTES);
        let charged = 2 * BYTES_PER_ITEM;
        assert!(
            added <= charged,
            "{added} bytes of pieces against {charged} bytes charged"
        );
        // The op's own charge is at least two items, which the bound above relies on.
        let insert = JsonOpContent::Text(JsonTextOp::Insert {
            pos: 0,
            text: "x".to_owned(),
        });
        assert!(op_work(&insert).total_items() >= 2);
    }

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
