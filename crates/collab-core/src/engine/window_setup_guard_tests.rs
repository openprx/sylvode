//! Round seven: the setup's allocation guard (`crate::setup_guard`), the shapes of the review's
//! R7-1 (style anchors and null-valued styles, which the work measure cannot see), the calibration
//! that the guard's limit comes from, and the other round-seven findings.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::cast_precision_loss
)]

use std::time::{Duration, Instant};

use loro::{ExportMode, LoroDoc};

use super::window_round2_tests::{flat_page_of, items_and_budget};
use super::window_round4_tests::first_text;
use super::window_round6_tests::{configure_styles, fragmented_page, mark_keys};
use super::{CollabEngine, LoroCollabEngine};
use crate::alloc_meter::requested_bytes;
use crate::facts::BaseFacts;
use crate::setup_guard::SetupGuard;

/// How a page's hostile texts fill their style state.
#[derive(Debug, Clone, Copy)]
pub(super) enum Anchors {
    /// Insert a character, mark it with `false` under the key `k`, delete it: an anchor pair stays.
    DeletedMarked,
    /// `bold` over the same single character, again and again with alternating values: nested
    /// same-key styles.
    NestedSameKey,
}

/// A flat page of `blocks` paragraphs whose first `hostile` texts hold `count` style marks of the
/// kind `shape` (anchors left by deleted marked text, or nested styles). Built as a snapshot.
pub(super) fn anchor_page(blocks: usize, hostile: usize, count: usize, shape: Anchors) -> Vec<u8> {
    let raw = LoroDoc::new();
    raw.import(&flat_page_of(blocks)).unwrap();
    raw.set_peer_id(48_500).unwrap();
    configure_styles(&raw, &["k".to_owned()]);
    for index in 0..hostile {
        let text = first_text(&raw, index);
        match shape {
            Anchors::DeletedMarked => {
                for _ in 0..count {
                    let end = text.len_unicode();
                    text.insert(end, "z").unwrap();
                    text.mark(end..end + 1, "k", false).unwrap();
                    text.delete(end, 1).unwrap();
                }
            }
            Anchors::NestedSameKey => {
                // The library skips a mark the range already has, so the value alternates.
                for step in 0..count {
                    text.mark(0..1, "bold", step % 2 == 0).unwrap();
                }
            }
        }
    }
    raw.commit();
    raw.export(ExportMode::snapshot()).unwrap()
}

/// [`fragmented_page`] with `unmark` (a null-valued style) in place of `mark(.., false)`.
pub(super) fn unmarked_page(blocks: usize, fragmented: usize, chars: usize, keys: usize) -> Vec<u8> {
    let raw = LoroDoc::new();
    raw.import(&flat_page_of(blocks)).unwrap();
    raw.set_peer_id(48_501).unwrap();
    let keys = mark_keys(keys);
    configure_styles(&raw, &keys);
    for index in 0..fragmented {
        let text = first_text(&raw, index);
        for _ in 0..chars {
            text.insert(0, "y").unwrap();
        }
        let len = text.len_unicode();
        for key in &keys {
            // `unmark` of a key the range does not have is skipped by the library.
            text.mark(0..len, key, true).unwrap();
            text.unmark(0..len, key).unwrap();
        }
    }
    raw.commit();
    raw.export(ExportMode::snapshot()).unwrap()
}

/// What the worker's setup of a page costs.
#[derive(Debug, Clone, Copy)]
pub(super) struct SetupCost {
    pub load_bytes: u64,
    pub prewarm_bytes: u64,
    pub compute_bytes: u64,
    pub total: Duration,
    pub has_facts: bool,
    pub fired: bool,
}

impl SetupCost {
    pub(super) const fn bytes(&self) -> u64 {
        self.load_bytes + self.prewarm_bytes + self.compute_bytes
    }
}

/// Runs the worker's setup (load, prewarm, facts) on `base` under a guard with `limit`, and
/// measures the allocation of each step and the time of the whole.
pub(super) fn setup_cost(base: &[u8], limit: u64) -> SetupCost {
    let started = Instant::now();
    let guard = SetupGuard::with_limit(limit);
    let before = requested_bytes();
    let engine = LoroCollabEngine::load(base).unwrap();
    let after_load = requested_bytes();
    engine.prewarm_for_writes();
    let after_prewarm = requested_bytes();
    let setup = BaseFacts::compute_guarded(&engine, &guard);
    SetupCost {
        load_bytes: after_load - before,
        prewarm_bytes: after_prewarm - after_load,
        compute_bytes: requested_bytes() - after_prewarm,
        total: started.elapsed(),
        has_facts: setup.facts.is_some(),
        fired: setup.guard_fired,
    }
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Allocation and time of the count of the items and of the projection that follows it, on a loaded
/// and prewarmed page.
fn print_stages(label: &str, base: &[u8]) {
    let engine = LoroCollabEngine::load(base).unwrap();
    engine.prewarm_for_writes();
    let guard = SetupGuard::with_limit(u64::MAX);
    let started = Instant::now();
    let before = requested_bytes();
    let counted = engine
        .projection_count_guarded(crate::work::item_budget_ceiling(), guard)
        .unwrap();
    let count_bytes = requested_bytes() - before;
    let count_time = started.elapsed();
    let started = Instant::now();
    let before = requested_bytes();
    engine.semantic_snapshot().unwrap();
    let projection_bytes = requested_bytes() - before;
    println!(
        "{label:<46} count {:>8.1} MiB {:>7.1} ms | projection {:>8.1} MiB {:>7.1} ms | within {}",
        mib(count_bytes),
        count_time.as_secs_f64() * 1e3,
        mib(projection_bytes),
        started.elapsed().as_secs_f64() * 1e3,
        matches!(counted, crate::engine::ProjectionCount::Within { .. }),
    );
}

fn print_cost(label: &str, base: &[u8]) {
    let cost = (0..2)
        .map(|_| setup_cost(base, u64::MAX))
        .min_by_key(|cost| cost.total)
        .unwrap();
    println!(
        "{label:<58} snapshot {:>7.2} MiB  load {:>8.1} MiB  prewarm {:>7.1} MiB  facts {:>8.1} MiB  total {:>8.1} MiB  setup {:>8.1} ms (x3.6 {:>8.1} ms)  facts {}",
        mib(base.len() as u64),
        mib(cost.load_bytes),
        mib(cost.prewarm_bytes),
        mib(cost.compute_bytes),
        mib(cost.bytes()),
        cost.total.as_secs_f64() * 1e3,
        cost.total.as_secs_f64() * 3.6e3,
        cost.has_facts,
    );
}

/// The measurements the guard's limit comes from. Run in release:
/// `cargo test --release -p collab-core --lib window_setup_guard_calibration -- --ignored --nocapture
/// --test-threads=1`.
#[test]
#[ignore = "calibration, run in release"]
fn window_setup_guard_calibration() {
    use super::window_round6_tests::{Formatting, formatted_page};
    println!("--- stages");
    print_stages("flat 9,950", &flat_page_of(9_950));
    print_stages("9,950 Clusters(2)", &formatted_page(9_950, Formatting::Clusters(2)));
    print_stages("9,950 Clusters(1)", &formatted_page(9_950, Formatting::Clusters(1)));
    print_stages(
        "fragmented 9,950, 24 x 4,000 x 15",
        &fragmented_page(9_950, 24, 4_000, 15),
    );
    print_stages(
        "anchors 1 x 20,000",
        &anchor_page(50, 1, 20_000, Anchors::DeletedMarked),
    );
    print_stages("unmark 1 x 4,000 x 100", &unmarked_page(50, 1, 4_000, 100));
    println!("--- honest pages");
    print_cost("flat 9,950 blocks", &flat_page_of(9_950));
    for (blocks, formatting) in [
        (9_950, Formatting::Clusters(1)),
        (9_950, Formatting::Clusters(2)),
        (9_950, Formatting::SixMarks),
        (2_000, Formatting::SixMarks),
        (1_200, Formatting::Clusters(2)),
    ] {
        print_cost(
            &format!("{blocks} blocks {formatting:?}"),
            &formatted_page(blocks, formatting),
        );
    }
    println!("--- hostile shapes, per text");
    for count in [2_500, 5_000, 10_000, 20_000] {
        print_cost(
            &format!("anchors of deleted marked text, 1 text x {count}"),
            &anchor_page(50, 1, count, Anchors::DeletedMarked),
        );
    }
    for count in [250, 500, 1_000, 2_000, 4_000] {
        print_cost(
            &format!("nested same-key, 1 text x {count}"),
            &anchor_page(50, 1, count, Anchors::NestedSameKey),
        );
    }
    for keys in [15, 100] {
        print_cost(
            &format!("unmark, 1 text x 4,000 pieces x {keys} keys"),
            &unmarked_page(50, 1, 4_000, keys),
        );
    }
    println!("--- R6-1");
    print_cost(
        "fragmented 240 x 4,000 x 15 (review's page)",
        &fragmented_page(240, 240, 4_000, 15),
    );
    print_cost(
        "fragmented 9,950 blocks, 24 x 4,000 x 15",
        &fragmented_page(9_950, 24, 4_000, 15),
    );
}

// ---------------------------------------------------------------------------------------------
// R7-1: the guard
// ---------------------------------------------------------------------------------------------

/// The facts of `base` under a guard with `limit`, set up as the worker does (load, prewarm).
fn guarded_setup(base: &[u8], limit: u64) -> (SetupCost, crate::facts::SetupFacts) {
    let cost = setup_cost(base, limit);
    let engine = LoroCollabEngine::load(base).unwrap();
    engine.prewarm_for_writes();
    let setup = BaseFacts::compute_guarded(&engine, &SetupGuard::with_limit(limit));
    (cost, setup)
}

/// A setup the guard stopped: no facts, flagged, and the work stopped near the limit, far under what
/// the setup costs without a guard.
fn assert_cut_off(label: &str, base: &[u8], limit: u64, slack: u64) {
    let uncapped = setup_cost(base, u64::MAX);
    assert!(uncapped.has_facts, "{label}: the page has facts without a guard");
    let (capped, setup) = guarded_setup(base, limit);
    println!(
        "{label}: uncapped {:.1} MiB, limit {:.1} MiB, capped {:.1} MiB",
        mib(uncapped.bytes()),
        mib(limit),
        mib(capped.bytes())
    );
    assert!(
        uncapped.bytes() > limit + slack,
        "{label}: the page must cost more than the limit"
    );
    assert!(setup.facts.is_none(), "{label}: the guard yields no facts");
    assert!(setup.guard_fired && capped.fired, "{label}: the guard says it fired");
    assert!(
        capped.compute_bytes <= limit + slack,
        "{label}: the setup went on to {} bytes past a limit of {limit}",
        capped.compute_bytes
    );
}

/// Style anchors stay in a text after the text they marked is deleted, and no conversion output shows
/// them: the work measure charges the page like an unmarked one, the guard stops its setup.
#[test]
fn r7_1_anchors_left_by_deleted_marked_text_are_cut_off_by_the_guard() {
    let hostile = anchor_page(50, 1, 4_000, Anchors::DeletedMarked);
    assert_eq!(
        items_and_budget(&hostile).0,
        items_and_budget(&flat_page_of(50)).0,
        "the measure sees no difference"
    );
    assert_cut_off("anchors", &hostile, 2 * 1024 * 1024, 4 * 1024 * 1024);
}

/// Many marks of one key over one range (nested styles): one attribute in the delta.
#[test]
fn r7_1_nested_same_key_styles_are_cut_off_by_the_guard() {
    let hostile = anchor_page(50, 8, 300, Anchors::NestedSameKey);
    let (items, _) = items_and_budget(&hostile);
    let plain = items_and_budget(&flat_page_of(50)).0;
    assert!(
        items - plain <= 16,
        "the measure charges {} items for 8 x 300 nested marks",
        items - plain
    );
    assert_cut_off("nested", &hostile, 8 * 1024 * 1024, 4 * 1024 * 1024);
}

/// Null-valued styles (`unmark`): the delta shows no attribute and the pieces are charged as bare, so
/// the measure's figure does not move with the number of keys, and the setup's work does.
#[test]
fn r7_1_null_styles_are_invisible_to_the_measure_and_cut_off_by_the_guard() {
    let few = unmarked_page(50, 1, 1_500, 15);
    let many = unmarked_page(50, 1, 1_500, 100);
    assert_eq!(
        items_and_budget(&few).0,
        items_and_budget(&many).0,
        "the measure ignores the keys"
    );
    let (few_cost, _) = guarded_setup(&few, u64::MAX);
    let (many_cost, _) = guarded_setup(&many, u64::MAX);
    assert!(
        many_cost.compute_bytes > 2 * few_cost.compute_bytes,
        "the setup's work grows with the keys: {} against {}",
        many_cost.compute_bytes,
        few_cost.compute_bytes
    );
    let many_texts = unmarked_page(50, 6, 1_000, 100);
    assert_cut_off("null styles", &many_texts, 16 * 1024 * 1024, 16 * 1024 * 1024);
}

/// The page-level shape of the review (R7-1, scenario A): many blocks, each with a little, a setup that
/// no single text makes long. The guard stops the whole setup, not a text.
#[test]
fn r7_1_many_hostile_texts_are_cut_off_long_before_their_total() {
    let hostile = anchor_page(150, 40, 3_000, Anchors::DeletedMarked);
    let limit = 24 * 1024 * 1024;
    assert_cut_off("40 hostile texts", &hostile, limit, 8 * 1024 * 1024);
    let uncapped = setup_cost(&hostile, u64::MAX);
    let (capped, _) = guarded_setup(&hostile, limit);
    assert!(
        capped.bytes() * 3 < uncapped.bytes(),
        "capped {} against uncapped {}",
        capped.bytes(),
        uncapped.bytes()
    );
}

/// R6-1: a fragmented text that sits inside the budget has facts, and the guard still bounds the
/// setup that computes them.
#[test]
fn r7_1_a_fragmented_page_inside_its_budget_is_cut_off_by_the_guard() {
    let base = super::window_round6_tests::near_budget_fragmented_page();
    assert_cut_off("fragmented", &base, 2 * 1024 * 1024, 16 * 1024 * 1024);
}

/// A fired guard is an ordinary outcome: no facts, no error, and the window of the page is the full
/// computation, which accepts what it accepts.
#[test]
fn r7_1_a_page_without_facts_because_of_the_guard_is_written_by_the_full_window() {
    let base = anchor_page(50, 1, 1_000, Anchors::DeletedMarked);
    let update = super::hand_encoded::raw_edit(&base, 48_600, |raw| {
        first_text(raw, 3).insert(0, "q").unwrap();
    });
    let mut engine = LoroCollabEngine::load(&base).unwrap();
    engine.prewarm_for_writes();
    let setup = BaseFacts::compute_guarded(&engine, &SetupGuard::with_limit(1024));
    assert!(setup.facts.is_none() && setup.guard_fired);
    crate::walk_counter::reset();
    let accepted = crate::window_check::run_window(&mut engine, None, &update).expect("accepted, not an error");
    assert!(accepted.report.is_none());
    assert_eq!(
        accepted.fallback,
        Some(crate::window_check::FallbackReason::FactsAbsent)
    );
    assert_eq!(
        crate::walk_counter::fallback_reasons().last().map(String::as_str),
        Some("FactsAbsent")
    );
}

/// The count starts before the load: a base whose load and prewarm alone pass the limit yields no
/// facts before a single text is counted.
#[test]
fn r7_1_the_guard_counts_the_load_and_the_prewarm() {
    let base = flat_page_of(2_000);
    let guard = SetupGuard::with_limit(1024 * 1024);
    let engine = LoroCollabEngine::load(&base).unwrap();
    engine.prewarm_for_writes();
    assert!(
        guard.spent() > 1024 * 1024,
        "the prewarm of 2,000 blocks allocates {}",
        guard.spent()
    );
    crate::walk_counter::reset();
    let setup = BaseFacts::compute_guarded(&engine, &guard);
    assert!(setup.facts.is_none() && setup.guard_fired);
    assert!(crate::walk_counter::delta_reads().is_empty(), "nothing was counted");
    // The same page without the load in the count: facts.
    let fresh = SetupGuard::with_limit(1024 * 1024 * 1024);
    assert!(BaseFacts::compute_guarded(&engine, &fresh).facts.is_some());
}

/// A loaded and prewarmed page.
struct LoadedPage(LoroCollabEngine);

impl LoadedPage {
    fn new(base: &[u8]) -> Self {
        let engine = LoroCollabEngine::load(base).unwrap();
        engine.prewarm_for_writes();
        Self(engine)
    }
}

/// The projection that follows the count converts the texts the count converted, so the guard holds
/// the limit against the count's cost twice: a limit the count passes but the projection would not is
/// stopped before the projection runs.
#[test]
fn r7_1_the_projection_after_the_count_is_predicted_from_the_count() {
    let base = super::window_round6_tests::formatted_page(300, super::window_round6_tests::Formatting::Clusters(2));
    let fresh = || {
        let engine = LoadedPage::new(&base);
        engine.0
    };
    // The cost of the count on a freshly loaded page (a second count would find the texts decoded).
    let probe_engine = fresh();
    let probe = SetupGuard::with_limit(u64::MAX);
    let _ = probe_engine
        .projection_count_guarded(crate::work::item_budget_ceiling(), probe)
        .unwrap();
    let count_bytes = probe.spent();
    let engine = fresh();
    // Between one and two counts: the count passes, the count plus its twin does not.
    let limit = count_bytes + count_bytes / 2;
    let guard = SetupGuard::with_limit(limit);
    let setup = BaseFacts::compute_guarded(&engine, &guard);
    assert!(
        setup.guard_fired && setup.facts.is_none(),
        "limit {limit}, count {count_bytes}"
    );
    assert!(
        guard.spent() <= count_bytes + 1024 * 1024,
        "the projection ran: {} bytes spent, the count cost {count_bytes}",
        guard.spent()
    );
    // And a limit above two counts and the projection holds.
    let roomy = SetupGuard::with_limit(count_bytes * 4);
    assert!(BaseFacts::compute_guarded(&fresh(), &roomy).facts.is_some());
}

/// The meter that the guard reads is fed in the test build.
#[test]
fn the_allocation_meter_is_installed_and_counts_what_a_thread_allocates() {
    assert!(crate::alloc_meter::is_installed());
    let before = requested_bytes();
    let kept = std::hint::black_box(vec![0_u8; 100_000]);
    assert!(requested_bytes() - before >= 100_000);
    drop(kept);
    // Frees are not subtracted, and another thread's allocation is not this thread's.
    let after = requested_bytes();
    std::thread::spawn(|| drop(std::hint::black_box(vec![0_u8; 1_000_000])))
        .join()
        .unwrap();
    // (Spawning costs the parent a few kilobytes; the child's megabyte is not among them.)
    assert!(requested_bytes() - after < 500_000);
}

/// Honest pages that are incremental stay so under the production limit, with a margin of 1.5 on
/// the whole setup (the largest honest shape of the calibration costs about half the limit).
#[test]
fn r7_1_the_largest_honest_formatted_pages_stay_far_under_the_production_limit() {
    use super::window_round6_tests::{Formatting, formatted_page};
    for formatting in [Formatting::Clusters(2), Formatting::SixMarks] {
        let base = formatted_page(crate::limits::DOCUMENT_BLOCK_COUNT_MAX - 50, formatting);
        let cost = setup_cost(&base, crate::setup_guard::SETUP_ALLOCATION_LIMIT_BYTES);
        println!(
            "{formatting:?}: {:.1} MiB of {:.1} MiB",
            mib(cost.bytes()),
            mib(crate::setup_guard::SETUP_ALLOCATION_LIMIT_BYTES)
        );
        assert!(cost.has_facts && !cost.fired, "{formatting:?}");
        assert!(
            cost.bytes() / 2 * 3 <= crate::setup_guard::SETUP_ALLOCATION_LIMIT_BYTES,
            "{formatting:?}: {} bytes leave no margin to the limit",
            cost.bytes()
        );
    }
}

/// Corpus extension of this round: a page the guard stops (its facts are absent) and one it does
/// not (a control with the same base), each with a typed character in the hostile text, in another
/// block and a mark. The decision is the full computation's either way; the mutant that never
/// fires the guard decides along another path.
pub(super) fn round7_cases(scale: u64) -> Vec<super::window_fixtures::Case> {
    use super::window_fixtures::{Case, FactsFrom};
    let base = std::sync::Arc::new(anchor_page(60, 2, 2_000, Anchors::DeletedMarked));
    let mut cases = Vec::new();
    for k in 0..scale {
        let typed_hostile = super::hand_encoded::raw_edit(&base, 49_700 + k, |raw| {
            first_text(raw, 0).insert(0, "h").unwrap();
        });
        let typed_other = super::hand_encoded::raw_edit(&base, 49_800 + k, |raw| {
            first_text(raw, 7).insert(1, "o").unwrap();
        });
        let marked_other = super::hand_encoded::raw_edit(&base, 49_900 + k, |raw| {
            let text = first_text(raw, 9);
            text.insert(1, "mm").unwrap();
            text.mark(1..2, "bold", true).unwrap();
        });
        for (kind, update) in [
            ("guarded_typed_hostile", typed_hostile),
            ("guarded_typed_other", typed_other),
            ("guarded_marked_other", marked_other),
        ] {
            for (name, limit) in [("fires", 1024 * 1024), ("holds", u64::MAX)] {
                cases.push(Case {
                    label: format!("round7/{kind}/{name}/{k}"),
                    family: "round7",
                    kind,
                    base: base.clone(),
                    update: update.clone(),
                    expect: None,
                    facts: FactsFrom::Guarded(limit),
                });
            }
        }
    }
    cases
}

/// The mutant that never fires the guard is observable: the page that the guard stops has facts.
#[test]
fn r7_1_the_mutant_without_the_guard_builds_facts_for_a_page_the_guard_stops() {
    let base = anchor_page(50, 1, 3_000, Anchors::DeletedMarked);
    let (_, setup) = guarded_setup(&base, 1024 * 1024);
    assert!(setup.facts.is_none() && setup.guard_fired);
    let _fault = crate::window_check::fault::inject(crate::window_check::fault::Fault::SetupGuardSkipped);
    let (_, setup) = guarded_setup(&base, 1024 * 1024);
    assert!(setup.facts.is_some() && !setup.guard_fired);
}

// ---------------------------------------------------------------------------------------------
// R7-2: the setup converts a marked text twice, not three times
// ---------------------------------------------------------------------------------------------

#[test]
fn r7_2_the_setup_counts_the_delta_of_every_marked_text_once_and_keeps_the_figures() {
    use super::window_round6_tests::{Formatting, formatted_page};
    let base = formatted_page(40, Formatting::Clusters(1));
    let engine = LoroCollabEngine::load(&base).unwrap();
    engine.prewarm_for_writes();
    crate::walk_counter::reset();
    let facts = BaseFacts::compute(&engine).expect("facts");
    let reads = crate::walk_counter::delta_reads();
    let mut distinct = reads.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(reads.len(), 40, "one count per marked text");
    assert_eq!(distinct.len(), reads.len(), "no text is converted twice");
    // The figures are the ones a pass of their own computes.
    assert_eq!(facts.marks, engine.unit_mark_items().unwrap());
    assert_eq!(facts.mark_items, facts.marks.values().sum::<u64>());
}

// ---------------------------------------------------------------------------------------------
// R7-4: counting a page does not decode the style state of an unread text
// ---------------------------------------------------------------------------------------------

/// A base whose `order` map holds one text of 3,000 characters: typed in reverse order under 60
/// full-range marks (`fragmented`), or inserted at once with no marks.
fn page_with_unread_text(fragmented: bool) -> Vec<u8> {
    let raw = LoroDoc::new();
    raw.import(&flat_page_of(50)).unwrap();
    raw.set_peer_id(48_400).unwrap();
    let keys = mark_keys(60);
    configure_styles(&raw, &keys);
    let text = raw
        .get_map("order")
        .insert_container("t", loro::LoroText::new())
        .unwrap();
    if fragmented {
        for _ in 0..3_000 {
            text.insert(0, "y").unwrap();
        }
        for key in &keys {
            text.mark(0..3_000, key, false).unwrap();
        }
    } else {
        text.insert(0, &"y".repeat(3_000)).unwrap();
    }
    raw.commit();
    raw.export(ExportMode::snapshot()).unwrap()
}

/// R5-1, the decode half: `len_unicode` of a lazily loaded text reads only its string (its chunks,
/// not its styles), while `iter` decodes its style state, so counting a page must not pay for the
/// pieces and styles of a text the projection does not read. Compared by allocation on two pages that
/// differ only in that text, with the cost of reading the text's length (which the count does pay)
/// allowed for, so no clock is involved.
#[test]
fn r7_4_counting_a_page_does_not_decode_the_style_state_of_an_unread_text() {
    let allocated = |fragmented: bool, work: &dyn Fn(&LoroCollabEngine)| {
        let engine = LoroCollabEngine::load(&page_with_unread_text(fragmented)).unwrap();
        let before = crate::test_alloc::allocated_bytes();
        work(&engine);
        crate::test_alloc::allocated_bytes() - before
    };
    let count = |engine: &LoroCollabEngine| {
        engine.projection_items(u64::MAX).unwrap().unwrap();
    };
    let length_only = |engine: &LoroCollabEngine| {
        let Some(loro::ValueOrContainer::Container(loro::Container::Text(text))) = engine.doc.get_map("order").get("t")
        else {
            panic!("no text")
        };
        let _ = text.len_unicode();
    };
    let (fragmented, plain) = (allocated(true, &count), allocated(false, &count));
    let length = allocated(true, &length_only);
    println!("count: fragmented {fragmented} B, plain {plain} B, the length of the text {length} B");
    assert!(
        fragmented <= plain + length + 4_096,
        "counting the page allocates {fragmented} bytes with the fragmented text and {plain} without \
         (reading its length accounts for {length}): the style state is decoded"
    );
}

/// The control of the test above: the decode is visible to the counter. Reading the fragmented
/// text's pieces allocates far more than reading its length.
#[test]
fn r7_4_the_decode_of_a_fragmented_text_is_visible_to_the_allocation_counter() {
    let read = |decode: bool| {
        let engine = LoroCollabEngine::load(&page_with_unread_text(true)).unwrap();
        let Some(loro::ValueOrContainer::Container(loro::Container::Text(text))) = engine.doc.get_map("order").get("t")
        else {
            panic!("no text")
        };
        let before = crate::test_alloc::allocated_bytes();
        if decode {
            text.iter(|_| true);
        } else {
            let _ = text.len_unicode();
        }
        crate::test_alloc::allocated_bytes() - before
    };
    let (decoded, length_only) = (read(true), read(false));
    println!("iter {decoded} B, len_unicode {length_only} B");
    assert!(
        decoded >= length_only + 3_000 * 16,
        "iter allocates {decoded} bytes, len_unicode {length_only}"
    );
}

// ---------------------------------------------------------------------------------------------
// R7-3: honest edit history
// ---------------------------------------------------------------------------------------------

/// [`formatted_page`](super::window_round6_tests::formatted_page) followed by `edits` rounds of
/// single-character edits in every block, at pseudo-random positions (an insert or a delete), made by
/// two peers in turn with their documents synchronised after every round: what a page that has been
/// edited by two people looks like, with several internal pieces per block.
pub(super) fn edited_formatted_page(
    blocks: usize,
    formatting: super::window_round6_tests::Formatting,
    edits: usize,
) -> Vec<u8> {
    let base = super::window_round6_tests::formatted_page(blocks, formatting);
    let docs = [LoroDoc::new(), LoroDoc::new()];
    for (index, doc) in docs.iter().enumerate() {
        doc.import(&base).unwrap();
        doc.set_peer_id(48_700 + index as u64).unwrap();
        crate::page_inline::configure(doc);
    }
    let mut rng = super::window_fixtures::Rng::new(7);
    for round in 0..edits {
        let (writer, other) = (&docs[round % 2], &docs[(round + 1) % 2]);
        let before = writer.oplog_vv();
        for block in 0..blocks {
            let text = first_text(writer, block);
            let length = text.len_unicode();
            let at = usize::try_from(rng.next() % u64::try_from(length.max(1)).unwrap()).unwrap();
            if rng.next().is_multiple_of(2) || length < 20 {
                text.insert(at, "e").unwrap();
            } else {
                text.delete(at, 1).unwrap();
            }
        }
        writer.commit();
        other
            .import(&writer.export(ExportMode::updates(&before)).unwrap())
            .unwrap();
    }
    docs[0].commit();
    docs[0].export(ExportMode::snapshot()).unwrap()
}

/// R7-3: edit history is charged by its pieces, and a page of ordinary size stays incremental.
#[test]
fn r7_3_an_edited_formatted_page_is_charged_for_its_pieces_and_stays_incremental() {
    use super::window_round6_tests::{Formatting, formatted_page};
    let plain = formatted_page(100, Formatting::Clusters(2));
    let edited = edited_formatted_page(100, Formatting::Clusters(2), 30);
    let (plain_items, _) = items_and_budget(&plain);
    let (edited_items, budget) = items_and_budget(&edited);
    println!(
        "100 blocks, 2 clusters: {plain_items} items unedited, {edited_items} with 30 edits per block, budget {budget}"
    );
    assert!(edited_items > plain_items, "the pieces of the edits are charged");
    assert!(edited_items <= budget);
    let cost = setup_cost(&edited, crate::setup_guard::SETUP_ALLOCATION_LIMIT_BYTES);
    assert!(cost.has_facts && !cost.fired);
}

/// R7-3, the measurement: formatted pages whose blocks carry the pieces of real edits. Items per
/// block against the budget, whether the page is incremental, and the setup. Run in release.
#[test]
#[ignore = "calibration, run in release"]
fn window_edit_history_calibration() {
    use super::window_round6_tests::Formatting;
    for (blocks, formatting, edits) in [
        (700, Formatting::Clusters(2), 30),
        (1_000, Formatting::Clusters(2), 30),
        (1_200, Formatting::Clusters(2), 30),
        (1_200, Formatting::Clusters(2), 10),
        (2_000, Formatting::SixMarks, 30),
        (3_000, Formatting::Clusters(1), 30),
    ] {
        let base = edited_formatted_page(blocks, formatting, edits);
        let (items, budget) = items_and_budget(&base);
        println!(
            "{blocks} blocks {formatting:?}, {edits} edits per block: {:.1} items per block, budget {:.1} per block, incremental {}",
            items as f64 / blocks as f64,
            budget as f64 / blocks as f64,
            items <= budget
        );
        print_cost(&format!("  setup {blocks} x {formatting:?} x {edits}"), &base);
    }
}
