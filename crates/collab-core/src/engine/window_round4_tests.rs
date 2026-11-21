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
/// Before the per-segment count, 80 of 80 writes were decided incrementally and the facts' items
/// stayed flat; now the facts count the payload once per segment and the window re-counts the
/// touched text, so the page leaves the incremental path at the budget.
#[test]
fn r4_2_a_large_mark_value_over_many_segments_cannot_grow_a_page_without_a_bound() {
    let (incremental, first_fallback, base) = grow_by(flat_page_of(100), 80, &|raw, write| {
        let text = first_text(raw, 0);
        if write == 0 {
            text.insert(1, &"y".repeat(1_999)).unwrap();
            text.mark(0..2_000, "bold", large_mark_value()).unwrap();
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

/// An object of 1,500 one-key objects: about 40 KB of payload, accepted by the reader as a value of
/// `bold` and `italic`.
fn large_mark_value() -> loro::LoroValue {
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
    loro::LoroValue::from(entries)
}

/// A flat page whose first paragraph is 2,000 characters under one `bold` mark with a large value,
/// then given one-character `italic` marks until one more would take the page over its work budget:
/// the base is within the budget, so it has facts, and the next mark that splits the text is not.
fn near_budget_page() -> Vec<u8> {
    let first = super::hand_encoded::raw_edit(&flat_page_of(100), 40_000, |raw| {
        let text = first_text(raw, 0);
        text.insert(1, &"y".repeat(1_999)).unwrap();
        text.mark(0..2_000, "bold", large_mark_value()).unwrap();
    });
    let mut base = super::hand_encoded::accepted(&flat_page_of(100), &first)
        .as_ref()
        .clone();
    for step in 0..400_u64 {
        let at = usize::try_from(step).unwrap() * 2;
        let update = super::hand_encoded::raw_edit(&base, 40_001 + step, |raw| {
            first_text(raw, 0).mark(at..at + 1, "italic", true).unwrap();
        });
        let candidate = super::hand_encoded::accepted(&base, &update).as_ref().clone();
        let (items, budget) = super::window_round2_tests::items_and_budget(&candidate);
        if items > budget {
            return base;
        }
        base = candidate;
    }
    panic!("the page never reached its budget");
}

/// The work budget decides before any unit is evaluated: one update puts 400 marks on a text with a
/// large value, so the segments it adds carry about 240,000 items of payload against a headroom of a
/// few thousand. The walk of the touched text stops at the headroom, and the evaluation of its
/// segments (the work the bound protects) never starts.
#[test]
fn r4_2_an_update_far_over_the_budget_is_refused_before_a_unit_is_evaluated() {
    let base = super::window_round2_tests::flat_page_of(100);
    let first = super::hand_encoded::raw_edit(&base, 42_100, |raw| {
        let text = first_text(raw, 0);
        text.insert(1, &"y".repeat(1_999)).unwrap();
        text.mark(0..2_000, "bold", large_mark_value()).unwrap();
    });
    let base = super::hand_encoded::accepted(&base, &first).as_ref().clone();
    let update = super::hand_encoded::raw_edit(&base, 42_101, |raw| {
        let text = first_text(raw, 0);
        for at in (0..800).map(|i| i * 2) {
            text.mark(at..at + 1, "italic", true).unwrap();
        }
    });
    let mut engine = LoroCollabEngine::load(&base).unwrap();
    engine.prewarm_for_writes();
    let facts = BaseFacts::compute(&engine);
    assert!(facts.is_some(), "the base must be within its budget");
    crate::walk_counter::reset();
    crate::window_check::run_window(&mut engine, facts.as_ref(), &update).unwrap();
    assert_eq!(
        crate::walk_counter::fallback_reasons().last().map(String::as_str),
        Some("WorkBudget")
    );
    assert_eq!(
        crate::walk_counter::window_counts().unit_evaluations,
        0,
        "a unit was evaluated before the budget decided"
    );
}

/// The same single mark, one character, on text far from the large value: the page is within its
/// budget after it only if the update's own bound says so, so the decision is the budget's either way
/// and the full computation accepts what `next` accepts.
#[test]
fn r4_2_the_decision_on_the_near_budget_page_is_next_s() {
    let base = near_budget_page();
    for (peer, at) in [(41_100, 1_001), (41_101, 1_501), (41_102, 3)] {
        let update = super::hand_encoded::raw_edit(&base, peer, |raw| {
            first_text(raw, 0).mark(at..at + 1, "italic", true).unwrap();
        });
        let judged = super::window_round2_tests::judge(&base, &update);
        judged.assert_sound(&format!("one more mark at {at}"));
    }
}

/// One update with many one-character marks: the segments all arrive at once.
#[test]
fn r4_2_many_marks_in_one_update_are_counted() {
    let base = super::window_round2_tests::flat_page_of(100);
    let first = super::hand_encoded::raw_edit(&base, 42_000, |raw| {
        let text = first_text(raw, 0);
        text.insert(1, &"y".repeat(1_999)).unwrap();
        text.mark(0..2_000, "bold", large_mark_value()).unwrap();
    });
    let base = super::hand_encoded::accepted(&base, &first).as_ref().clone();
    let update = super::hand_encoded::raw_edit(&base, 42_001, |raw| {
        let text = first_text(raw, 0);
        for at in (0..400).map(|i| i * 2) {
            text.mark(at..at + 1, "italic", true).unwrap();
        }
    });
    let judged = super::window_round2_tests::judge(&base, &update);
    judged.assert_sound("400 marks in one update");
    assert!(!judged.incremental, "{judged:?}");
    assert_eq!(judged.fallback(), Some("WorkBudget"));
}

/// A legacy node's text is read by the projection as a plain string (`to_string`), so its marks cost
/// the projection nothing and are not counted: the same growth that leaves the block page the
/// incremental path leaves this one on it, and every decision is `next`'s.
#[test]
fn r4_2_marks_on_a_legacy_node_text_are_not_charged_because_the_projection_does_not_read_them() {
    let mut engine = LoroCollabEngine::new_empty(72);
    engine
        .apply_content_operation(&crate::operation::Operation::CreateNode {
            id: "node".into(),
            parent: None,
            index: 0,
            kind: crate::operation::NodeKind::NavigatorNode,
        })
        .unwrap();
    let node = *engine.id_to_tree.get("node").unwrap();
    let (incremental, first_fallback, base) = grow_by(engine.export_snapshot().unwrap(), 40, &|raw, write| {
        let text = raw
            .get_tree("tree")
            .get_meta(node)
            .unwrap()
            .ensure_mergeable_text("text")
            .unwrap();
        if write == 0 {
            text.insert(0, &"y".repeat(2_000)).unwrap();
            text.mark(0..2_000, "bold", large_mark_value()).unwrap();
        } else {
            let at = usize::try_from(write).unwrap() * 2;
            text.mark(at..at + 1, "italic", true).unwrap();
        }
    });
    assert_eq!((incremental, first_fallback), (40, None));
    let (items, budget) = super::window_round2_tests::items_and_budget(&base);
    assert!(items <= budget, "{items} > {budget}");
}

/// Ordinary marks (`bold`, `italic` and a `link` with a string value) on every block of a page at the
/// block limit stay within the budget, and a one-character edit is still decided incrementally.
#[test]
fn ordinary_marks_on_a_page_at_the_block_limit_stay_incremental() {
    let blocks = crate::limits::DOCUMENT_BLOCK_COUNT_MAX - 50;
    let base = flat_page_of(blocks);
    // Built on a raw copy and taken as a snapshot: the update would be far over the update limit.
    let raw = LoroDoc::new();
    raw.import(&base).unwrap();
    raw.set_peer_id(43_000).unwrap();
    {
        let raw = &raw;
        for index in 0..blocks {
            let text = first_text(raw, index);
            text.insert(1, "abcdefghijklmn").unwrap();
            text.mark(0..6, "bold", true).unwrap();
            text.mark(4..9, "italic", true).unwrap();
            text.mark(8..12, "link", "https://example.test/a/page").unwrap();
        }
    }
    raw.commit();
    let base = raw.export(loro::ExportMode::snapshot()).unwrap();
    let (items, budget) = super::window_round2_tests::items_and_budget(&base);
    println!("ordinary marks at {blocks} blocks: items {items}, budget {budget}");
    assert!(items <= budget);
    let update = super::hand_encoded::raw_edit(&base, 43_001, |raw| {
        first_text(raw, 5_000).insert(2, "z").unwrap();
    });
    let judged = super::window_round2_tests::judge(&base, &update);
    judged.assert_sound("an edit on a page with ordinary marks");
    assert!(judged.incremental, "{judged:?}");
    let marked_again = super::hand_encoded::raw_edit(&base, 43_002, |raw| {
        first_text(raw, 7_000).mark(10..11, "bold", true).unwrap();
    });
    let judged = super::window_round2_tests::judge(&base, &marked_again);
    judged.assert_sound("a mark on a page with ordinary marks");
    assert!(judged.incremental, "{judged:?}");
}

// ---------------------------------------------------------------------------------------------
// The corpus extension
// ---------------------------------------------------------------------------------------------

/// One more mark, and one more character, on the near-budget page: the decision is the work
/// budget's, which the mutant that skips the re-count gets wrong.
pub(super) fn round4_cases(scale: u64) -> Vec<super::window_fixtures::Case> {
    use super::window_fixtures::{Case, FactsFrom};
    let base = std::sync::Arc::new(near_budget_page());
    let mut cases = Vec::new();
    for k in 0..scale {
        let at = usize::try_from(1_001 + 2 * k).unwrap();
        let mark = super::hand_encoded::raw_edit(&base, 44_000 + k, |raw| {
            first_text(raw, 0).mark(at..at + 1, "italic", true).unwrap();
        });
        let typed = super::hand_encoded::raw_edit(&base, 44_500 + k, |raw| {
            first_text(raw, 0).insert(at, "q").unwrap();
        });
        for (kind, update) in [("mark_over_budget", mark), ("insert_in_marked_text", typed)] {
            cases.push(Case {
                label: format!("round4/{kind}/{k}"),
                family: "round4",
                kind,
                base: base.clone(),
                update,
                expect: None,
                facts: FactsFrom::Base,
            });
        }
    }
    cases
}

/// Full and incremental window of a one-character edit on flat pages, with and without ordinary
/// marks on every block. Run: `cargo test --release -p collab-core --lib mark_window_timing --
/// --ignored --nocapture`.
#[test]
#[ignore = "timing measurement, run in release"]
fn mark_window_timing() {
    for (blocks, marks) in [(5_000, false), (5_000, true), (9_950, false), (9_950, true)] {
        let base = flat_page_of(blocks);
        let raw = LoroDoc::new();
        raw.import(&base).unwrap();
        raw.set_peer_id(45_000).unwrap();
        if marks {
            for index in 0..blocks {
                let text = first_text(&raw, index);
                text.insert(1, "abcdefghijklmn").unwrap();
                text.mark(0..6, "bold", true).unwrap();
                text.mark(4..9, "italic", true).unwrap();
                text.mark(8..12, "link", "https://example.test/a/page").unwrap();
            }
            raw.commit();
        }
        let base = raw.export(loro::ExportMode::snapshot()).unwrap();
        let update = super::hand_encoded::raw_edit(&base, 45_001, |raw| {
            first_text(raw, blocks / 2).insert(1, "z").unwrap();
        });
        let time = |with_facts: bool| {
            let mut best = std::time::Duration::MAX;
            for _ in 0..5 {
                let mut engine = LoroCollabEngine::load(&base).unwrap();
                engine.prewarm_for_writes();
                let facts = if with_facts { BaseFacts::compute(&engine) } else { None };
                assert_eq!(facts.is_some(), with_facts);
                let started = std::time::Instant::now();
                crate::window_check::run_window(&mut engine, facts.as_ref(), &update).unwrap();
                best = best.min(started.elapsed());
            }
            best
        };
        println!(
            "TIMING {blocks} blocks, marks {marks}: full {:.2} ms, incremental {:.3} ms",
            time(false).as_secs_f64() * 1e3,
            time(true).as_secs_f64() * 1e3
        );
    }
}
