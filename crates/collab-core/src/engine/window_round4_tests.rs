//! Regression tests for the fourth review of the incremental write window.
//!
//! Same method as the round-three tests: inputs are built from raw documents and the window runs as
//! the worker does (see `window_round2_tests`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use loro::{Container, LoroDoc, LoroText, ValueOrContainer};

use super::window_round2_tests::{flat_page_of, grow_by};
use super::{CollabEngine, LoroCollabEngine};
use crate::facts::BaseFacts;

// ---------------------------------------------------------------------------------------------
// R4-1: the payload of the navigator `order` map is charged
// ---------------------------------------------------------------------------------------------

/// Each write inserts one `order` entry whose value is a string of 60,000 characters. The server's
/// own writer bounds such a key to 65 characters, a client update does not, and the full projection
/// clones every value.
#[test]
fn r4_1_order_values_cannot_grow_a_flat_page_without_a_bound() {
    let (incremental, first_fallback, base) = grow_by(flat_page_of(50), 60, &|raw, write| {
        raw.get_map("order")
            .insert(&format!("n{write}"), "a".repeat(60_000))
            .unwrap();
    });
    println!("order values: {incremental} incremental writes, then {first_fallback:?}");
    assert!(incremental >= 1);
    assert_eq!(first_fallback.as_deref(), Some("WorkBudget"));
    assert!(incremental <= 30, "{incremental} writes of 60 KB each");
    assert!(BaseFacts::compute(&LoroCollabEngine::load(&base).unwrap()).is_none());
}

/// The same through the keys: one entry per write whose key is 60,000 characters.
#[test]
fn r4_1_order_keys_cannot_grow_a_flat_page_without_a_bound() {
    let (incremental, first_fallback, _) = grow_by(flat_page_of(50), 60, &|raw, write| {
        raw.get_map("order")
            .insert(&format!("{write}{}", "k".repeat(60_000)), "a")
            .unwrap();
    });
    println!("order keys: {incremental} incremental writes, then {first_fallback:?}");
    assert_eq!(first_fallback.as_deref(), Some("WorkBudget"));
    assert!(incremental <= 30, "{incremental} writes of 60 KB each");
}

/// Alias values are read per resolved block, but the count is uniform.
#[test]
fn r4_1_alias_values_are_charged_by_their_payload() {
    let (incremental, first_fallback, _) = grow_by(flat_page_of(50), 60, &|raw, write| {
        raw.get_map(crate::reserved_roots::ALIASES)
            .insert(&format!("n{write}"), "a".repeat(60_000))
            .unwrap();
    });
    println!("alias values: {incremental} incremental writes, then {first_fallback:?}");
    assert_eq!(first_fallback.as_deref(), Some("WorkBudget"));
    assert!(incremental <= 30, "{incremental} writes of 60 KB each");
}

// ---------------------------------------------------------------------------------------------
// R4-2: the payload of text mark values, read once per delta segment
// ---------------------------------------------------------------------------------------------

/// The first inline text of block `index`.
fn first_text(raw: &LoroDoc, index: usize) -> LoroText {
    let list = super::floor_equivalence_tests::children(raw);
    let Some(ValueOrContainer::Container(Container::Map(block))) = list.get(index) else {
        panic!("no block at {index}")
    };
    let Some(ValueOrContainer::Container(Container::List(inline))) = block.get("children") else {
        panic!("no inline list")
    };
    for position in 0..inline.len() {
        if let Some(ValueOrContainer::Container(Container::Text(text))) = inline.get(position) {
            return text;
        }
    }
    panic!("no inline text")
}

/// One large `strong` value over a paragraph, then one small `em` mark per write on alternating
/// characters. Every `em` mark splits the text once more, and the full projection converts the large
/// value again for every segment (`page_inline::spans`), so what the page costs grows with segments
/// times payload while the facts counted a mark's payload once or not at all.
///
/// Reproduces today (80 of 80 writes are decided incrementally, the facts' items stay flat) and is
/// ignored because no work-budget term can close it: the payload of a mark is read once per delta
/// segment, and the number of segments an update adds depends on the text it lands in (an `italic`
/// mark of one character splits a text that has a large `bold` value into two more segments, each
/// converting the large value again), which an op alone does not state. Charging the payload per
/// delta segment on the base side is possible, but the update side would need the touched texts'
/// post-import segments, so the facts would have to carry a per-unit mark payload.
#[test]
#[ignore = "R4-2 is open: byte-charging cannot bound segments times payload, see the test's documentation"]
fn r4_2_a_large_mark_value_over_many_segments_cannot_grow_a_page_without_a_bound() {
    let (incremental, first_fallback, base) = grow_by(flat_page_of(100), 80, &|raw, write| {
        let text = first_text(raw, 0);
        if write == 0 {
            text.insert(1, &"y".repeat(1_999)).unwrap();
            let entries: std::collections::HashMap<String, loro::LoroValue> = (0..1_500)
                .map(|i| {
                    (
                        format!("k{i}"),
                        loro::LoroValue::from(std::collections::HashMap::from([(
                            "v".to_owned(),
                            loro::LoroValue::from(i64::from(i)),
                        )])),
                    )
                })
                .collect();
            text.mark(0..2_000, "bold", loro::LoroValue::from(entries)).unwrap();
        } else {
            let at = usize::try_from(write).unwrap() * 2;
            text.mark(at..at + 1, "italic", true).unwrap();
        }
    });
    println!("mark values: {incremental} incremental writes, then {first_fallback:?}");
    assert!(incremental >= 1);
    assert_eq!(first_fallback.as_deref(), Some("WorkBudget"));
    assert!(incremental <= 60, "{incremental} writes");
    assert!(BaseFacts::compute(&LoroCollabEngine::load(&base).unwrap()).is_none());
}

// ---------------------------------------------------------------------------------------------
// The refusal of an id rewrite: a legitimate client must not hit it
// ---------------------------------------------------------------------------------------------

/// The page undo manager of the editor covers the whole document, so it also records the creation
/// of a navigator node (a tree create and the write of its `logical_id`, one commit). Neither the
/// undo nor the redo of that commit may be refused as a rewrite of an existing node's id: they are
/// decided like any other update. Run with the library's own undo manager; the frontend's pinned
/// `loro-crdt` build is not exercised here.
#[test]
fn undo_and_redo_of_a_node_creation_are_not_refused_as_an_id_rewrite() {
    let mut engine = LoroCollabEngine::new_empty(71);
    engine
        .apply_content_operation(&crate::operation::Operation::CreateNode {
            id: "anchor".into(),
            parent: None,
            index: 0,
            kind: crate::operation::NodeKind::NavigatorNode,
        })
        .unwrap();
    let base0 = engine.export_snapshot().unwrap();

    let author = LoroDoc::new();
    author.import(&base0).unwrap();
    author.set_peer_id(7_100).unwrap();
    let mut undo = loro::UndoManager::new(&author);
    let tree = author.get_tree("tree");
    tree.enable_fractional_index(0);
    let from = author.oplog_vv();
    let created = tree.create(loro::TreeParentId::Root).unwrap();
    super::hand_encoded::set_node(&author, created, "fresh", "navigator_node");
    author.commit();
    let creation = author.export(loro::ExportMode::updates(&from)).unwrap();
    let mut base = super::hand_encoded::accepted(&base0, &creation).as_ref().clone();

    for (label, step) in [("undo", true), ("redo", false)] {
        let from = author.oplog_vv();
        let moved = if step { undo.undo() } else { undo.redo() }.unwrap();
        assert!(moved, "{label} had nothing to do");
        author.commit();
        let update = author.export(loro::ExportMode::updates(&from)).unwrap();
        let judged = super::window_round2_tests::judge(&base, &update);
        println!("{label}: {judged:?}");
        judged.assert_sound(label);
        assert!(
            !matches!(&judged.window, Err(reason) if reason.contains(crate::window_check::ID_REWRITE_REASON)),
            "{label} was refused as an id rewrite: {judged:?}"
        );
        base = super::hand_encoded::accepted(&base, &update).as_ref().clone();
    }
}
