//! Round eight: the per-text budget of the setup (`crate::setup_guard`), the review's A-1 growth
//! scenario, and the calibration the per-text allocation budget comes from.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use loro::{ExportMode, LoroDoc};

use super::window_fixtures::{Case, FactsFrom};
use super::window_round2_tests::{flat_page_of, items_and_budget};
use super::window_round4_tests::first_text;
use super::window_setup_guard_tests::{Anchors, SetupCost, anchor_page, setup_cost_under};
use super::{CollabEngine, LoroCollabEngine};
use crate::alloc_meter::requested_bytes;
use crate::facts::BaseFacts;
use crate::setup_guard::{
    Fired, SETUP_ALLOCATION_LIMIT_BYTES, SetupGuard, TEXT_ALLOCATION_LIMIT_BYTES, TEXT_CPU_BUDGET_NANOS,
    stepping_clock, thread_cpu_nanos,
};
use crate::window_check::fault::{Fault, inject};

/// How an honest text of the documented limit is built.
#[derive(Debug, Clone, Copy)]
pub(super) enum Honest {
    /// 100,000 characters inserted at once, no formatting.
    Plain,
    /// 100,000 characters, then `edits` single-character inserts and deletes at pseudo-random
    /// positions by two peers in turn with their documents synchronised after every round.
    Edited(usize),
    /// 100,000 characters under `clusters` clusters of bold, italic and a link (30 characters
    /// apart), the way `window_round6_tests::formatted_page` formats a block.
    Formatted(usize),
    /// `Formatted(clusters)` followed by `edits` edits.
    FormattedEdited(usize, usize),
    /// The first `pieces` characters typed in reverse order (one internal piece per character), the
    /// rest inserted at once: the heaviest real edit history a page can hold within its budget.
    Reversed(usize),
}

/// [`build_honest_text_page`], built once per shape for all the tests of the process: the edit
/// history is replayed between two documents, which takes seconds.
pub(super) fn honest_text_page(shape: Honest) -> Vec<u8> {
    static PAGES: std::sync::LazyLock<parking_lot::Mutex<std::collections::HashMap<String, Vec<u8>>>> =
        std::sync::LazyLock::new(Default::default);
    let mut pages = PAGES.lock();
    pages
        .entry(format!("{shape:?}"))
        .or_insert_with(|| build_honest_text_page(shape))
        .clone()
}

/// A flat page of one block whose text is the largest the limits allow (100,000 characters),
/// built as `shape` says. Built as a snapshot.
fn build_honest_text_page(shape: Honest) -> Vec<u8> {
    const CHARS: usize = crate::limits::TEXT_BLOCK_CHARS_MAX;
    let base = flat_page_of(1);
    let docs = [LoroDoc::new(), LoroDoc::new()];
    for (index, doc) in docs.iter().enumerate() {
        doc.import(&base).unwrap();
        doc.set_peer_id(48_900 + index as u64).unwrap();
        crate::page_inline::configure(doc);
    }
    let raw = &docs[0];
    let text = first_text(raw, 0);
    let link = "https://example.test/a/page";
    let mut length = text.len_unicode();
    let body = |n: usize| "abcdefghij".repeat(n.div_ceil(10))[..n].to_owned();
    // A history of edits starts below the limit, so that the text stays within it.
    let start = if matches!(shape, Honest::Edited(_) | Honest::FormattedEdited(..)) {
        CHARS - 200
    } else {
        CHARS
    };
    match shape {
        Honest::Plain | Honest::Edited(_) => {
            text.insert(length, &body(start - length)).unwrap();
        }
        Honest::Formatted(clusters) | Honest::FormattedEdited(clusters, _) => {
            text.insert(length, &body(start - length)).unwrap();
            for cluster in 0..clusters {
                let start = cluster * 30;
                text.mark(start..start + 10, "bold", true).unwrap();
                text.mark(start + 5..start + 15, "italic", true).unwrap();
                text.mark(start + 8..start + 20, "link", link).unwrap();
            }
        }
        Honest::Reversed(pieces) => {
            for _ in 0..pieces {
                text.insert(0, "y").unwrap();
            }
            length = text.len_unicode();
            text.insert(length, &body(CHARS - length)).unwrap();
        }
    }
    raw.commit();
    if let Honest::Edited(edits) | Honest::FormattedEdited(_, edits) = shape {
        docs[1].import(&raw.export(ExportMode::snapshot()).unwrap()).unwrap();
        let mut rng = super::window_fixtures::Rng::new(11);
        for round in 0..edits {
            let (writer, other) = (&docs[round % 2], &docs[(round + 1) % 2]);
            let before = writer.oplog_vv();
            let text = first_text(writer, 0);
            let length = text.len_unicode();
            let at = usize::try_from(rng.next() % u64::try_from(length.max(1)).unwrap()).unwrap();
            if length < CHARS - 20 && rng.next().is_multiple_of(2) {
                text.insert(at, "e").unwrap();
            } else {
                text.delete(at, 1).unwrap();
            }
            writer.commit();
            other
                .import(&writer.export(ExportMode::updates(&before)).unwrap())
                .unwrap();
        }
    }
    docs[0].commit();
    docs[0].export(ExportMode::snapshot()).unwrap()
}

/// What the guard's per-text measure sees of the first text of a freshly loaded page: the thread CPU
/// time and the allocation of its decode and conversion (`work::text_mark_items`), in nanoseconds
/// and bytes.
pub(super) fn text_cost(base: &[u8]) -> (u64, u64) {
    let engine = LoroCollabEngine::load(base).unwrap();
    engine.prewarm_for_writes();
    let text = first_text(&engine.doc, 0);
    let (cpu_before, bytes_before) = (thread_cpu_nanos().unwrap(), requested_bytes());
    crate::work::text_mark_items(&text, u64::MAX, None).ok().unwrap();
    (
        thread_cpu_nanos().unwrap().saturating_sub(cpu_before),
        requested_bytes() - bytes_before,
    )
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn print_text(label: &str, base: &[u8]) {
    let (cpu, bytes) = (0..3).map(|_| text_cost(base)).min_by_key(|(cpu, _)| *cpu).unwrap();
    let (items, budget) = items_and_budget(base);
    println!(
        "{label:<52} snapshot {:>7.2} MiB  alloc {:>8.2} MiB  cpu {:>8.3} ms (x3.6 {:>8.3} ms)  items {items:>6} / budget {budget:>6}",
        mib(base.len() as u64),
        mib(bytes),
        cpu as f64 / 1e6,
        cpu as f64 * 3.6 / 1e6,
    );
}

/// The measurements the per-text budgets come from. Run in release:
/// `cargo test --release -p collab-core --lib window_text_guard_calibration -- --ignored --nocapture
/// --test-threads=1`.
#[test]
#[ignore = "calibration, run in release"]
fn window_text_guard_calibration() {
    println!("--- honest texts of 100,000 characters (the limit of one block)");
    print_text("plain", &honest_text_page(Honest::Plain));
    for edits in [1_000, 5_000, 20_000] {
        print_text(
            &format!("{edits} edits by two peers"),
            &honest_text_page(Honest::Edited(edits)),
        );
    }
    for clusters in [100, 500, 1_000, 2_000, 3_000] {
        print_text(
            &format!("{clusters} clusters (3 marks each)"),
            &honest_text_page(Honest::Formatted(clusters)),
        );
    }
    for (clusters, edits) in [(500, 5_000), (1_000, 5_000)] {
        print_text(
            &format!("{clusters} clusters, {edits} edits"),
            &honest_text_page(Honest::FormattedEdited(clusters, edits)),
        );
    }
    for pieces in [5_000, 10_000, 20_000, 25_000] {
        print_text(
            &format!("{pieces} reverse-typed pieces"),
            &honest_text_page(Honest::Reversed(pieces)),
        );
    }
    println!("--- hostile: nested same-key marks on one character");
    for count in [250, 500, 830, 1_000, 1_500, 2_000, 4_000] {
        print_text(
            &format!("nested x {count}"),
            &anchor_page(1, 1, count, Anchors::NestedSameKey),
        );
    }
    println!("--- hostile: anchors of deleted marked text");
    for count in [5_000, 20_000] {
        print_text(
            &format!("anchors x {count}"),
            &anchor_page(1, 1, count, Anchors::DeletedMarked),
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Calibration of the growth one window can add to a text, and of the worst setups under the guard
// ---------------------------------------------------------------------------------------------

/// The reference runner's factor over the development machine.
const REFERENCE_FACTOR: f64 = 3.6;

/// A flat page of `blocks` paragraphs whose first `hostile` texts hold `count` nested `bold` marks
/// each, for the tests of the worker process.
#[cfg(target_os = "linux")]
pub fn nested_marks_page(blocks: usize, hostile: usize, count: usize) -> Vec<u8> {
    anchor_page(blocks, hostile, count, Anchors::NestedSameKey)
}

/// A loaded and prewarmed page whose texts the setup has decoded (the count of the facts), as the
/// worker hands it to the window.
fn decoded_page(base: &[u8]) -> (LoroCollabEngine, BaseFacts) {
    let engine = LoroCollabEngine::load(base).unwrap();
    engine.prewarm_for_writes();
    let facts = BaseFacts::compute_guarded(&engine, &SetupGuard::with_limit(u64::MAX))
        .facts
        .expect("facts");
    (engine, facts)
}

/// An update by a fresh peer that adds `count` nested `bold` marks to the first text of a page that
/// already holds `existing` of them.
fn more_nested_marks(base: &[u8], peer: u64, existing: usize, count: usize) -> Vec<u8> {
    super::hand_encoded::raw_edit(base, peer, |raw| {
        let text = first_text(raw, 0);
        for step in existing..existing + count {
            text.mark(0..1, "bold", step % 2 == 0).unwrap();
        }
    })
}

/// How much one window can grow a text: from a page whose text holds `existing` nested marks, the
/// window of an update with `count` more, its CPU time (the window of the worker is bound to 50 ms of
/// it), the size of the update (bound to 64 KiB), and what the text costs the setup afterwards.
/// Run in release:
/// `cargo test --release -p collab-core --lib window_text_growth_bound -- --ignored --nocapture
/// --test-threads=1`.
#[test]
#[ignore = "calibration, run in release"]
fn window_text_growth_bound() {
    println!(
        "the text budget here is 50 ms of CPU, which is {:.1} ms on this machine for the reference runner's 50 ms",
        50.0 / REFERENCE_FACTOR
    );
    for existing in [400_usize, 500, 800, 830] {
        let base = anchor_page(1, 1, existing, Anchors::NestedSameKey);
        let (cpu0, bytes0) = text_cost(&base);
        println!(
            "--- a text of {existing} nested marks: setup cost {:.2} ms, {:.2} MiB",
            cpu0 as f64 / 1e6,
            bytes0 as f64 / 1_048_576.0
        );
        let mut count = 25;
        while count <= 12_800 {
            let update = more_nested_marks(&base, 49_000 + count as u64, existing, count);
            let (mut engine, facts) = decoded_page(&base);
            let before = thread_cpu_nanos().unwrap();
            let accepted = crate::window_check::run_window(&mut engine, Some(&facts), &update, &mut |_| {});
            let window_ms = (thread_cpu_nanos().unwrap() - before) as f64 / 1e6;
            let grown = engine.export_snapshot().unwrap();
            let (cpu1, bytes1) = text_cost(&grown);
            println!(
                "  +{count:>6} marks: update {:>6} B ({}), window {window_ms:>8.2} ms ({}), text then {:>8.2} ms ({:.2}x), {:.2} MiB",
                update.len(),
                if update.len() <= crate::limits::UPDATE_BYTES_MAX {
                    "within 64 KiB"
                } else {
                    "over 64 KiB"
                },
                if accepted.is_ok() { "accepted" } else { "refused" },
                cpu1 as f64 / 1e6,
                cpu1 as f64 / cpu0 as f64,
                bytes1 as f64 / 1_048_576.0,
            );
            if window_ms > 50.0 || update.len() > crate::limits::UPDATE_BYTES_MAX {
                break;
            }
            count *= 2;
        }
    }
}

/// The worst setup a page of many texts can cost under the production guard, in the reference
/// runner's units: the guard's budgets are scaled down by the reference factor, and the time of the
/// setup is scaled up by it. Run in release:
/// `cargo test --release -p collab-core --lib window_text_guard_worst_setup -- --ignored --nocapture
/// --test-threads=1`.
#[test]
#[ignore = "calibration, run in release"]
fn window_text_guard_worst_setup() {
    let page_budget = (2_500_000_000.0 / REFERENCE_FACTOR) as u64;
    let text_budget = (TEXT_CPU_BUDGET_NANOS as f64 / REFERENCE_FACTOR) as u64;
    for (blocks, hostile, count) in [
        (60_usize, 60_usize, 250_usize),
        (250, 250, 250),
        (250, 250, 380),
        (250, 250, 450),
        (250, 250, 500),
        (250, 250, 520),
    ] {
        let base = anchor_page(blocks, hostile, count, Anchors::NestedSameKey);
        let uncapped = setup_cost_under(&base, || SetupGuard::with_limit(u64::MAX));
        let guarded = setup_cost_under(&base, || {
            SetupGuard::with_limits(SETUP_ALLOCATION_LIMIT_BYTES, page_budget, thread_cpu_nanos)
                .with_text_limits(TEXT_ALLOCATION_LIMIT_BYTES, text_budget)
        });
        let (one, _) = text_cost(&base);
        println!(
            "{hostile} texts x {count} nested: one text {:.2} ms here ({:.1} ms ref, per-text budget {:.1} ms ref); setup without a guard {:.0} ms here ({:.0} ms ref); under the guard {:.0} ms here ({:.0} ms ref), fired {:?}, facts {}",
            one as f64 / 1e6,
            one as f64 * REFERENCE_FACTOR / 1e6,
            TEXT_CPU_BUDGET_NANOS as f64 / 1e6,
            uncapped.total.as_secs_f64() * 1e3,
            uncapped.total.as_secs_f64() * 1e3 * REFERENCE_FACTOR,
            guarded.total.as_secs_f64() * 1e3,
            guarded.total.as_secs_f64() * 1e3 * REFERENCE_FACTOR,
            guarded.fired,
            guarded.has_facts,
        );
    }
}

// ---------------------------------------------------------------------------------------------
// A-1: the per-text budget
// ---------------------------------------------------------------------------------------------

/// The deterministic guard of the tests: the production per-text allocation limit and page
/// allocation limit, no CPU clock.
fn allocation_guard() -> SetupGuard {
    SetupGuard::with_limit(SETUP_ALLOCATION_LIMIT_BYTES).with_text_limits(TEXT_ALLOCATION_LIMIT_BYTES, u64::MAX)
}

fn loaded(base: &[u8]) -> LoroCollabEngine {
    let engine = LoroCollabEngine::load(base).unwrap();
    engine.prewarm_for_writes();
    engine
}

/// The review's page: one text whose nested same-key marks cost more than a window ever admitted,
/// on a page that is far inside every page-level budget.
fn one_text_over_budget() -> Vec<u8> {
    anchor_page(50, 1, 1_800, Anchors::NestedSameKey)
}

/// A text that alone costs the setup more than a window's ceiling yields no facts, whatever the rest
/// of the page costs, and the guard says it was the text. The count stops at that text.
#[test]
fn r8_1_a_text_over_its_budget_yields_no_facts_and_the_guard_names_the_text() {
    let base = one_text_over_budget();
    // The page-level budgets alone do not stop this setup: the text is the whole cost.
    let page_only = SetupGuard::with_limit(SETUP_ALLOCATION_LIMIT_BYTES);
    let setup = BaseFacts::compute_guarded(&loaded(&base), &page_only);
    assert!(
        setup.facts.is_some() && setup.guard.is_none(),
        "the review's page has facts"
    );
    // With the production guard (real clock, production budgets) it has none.
    crate::walk_counter::reset();
    let setup = BaseFacts::compute_guarded(&loaded(&base), &SetupGuard::start());
    assert!(setup.facts.is_none());
    assert_eq!(setup.guard, Some(Fired::Text));
    assert_eq!(
        crate::walk_counter::delta_reads().len(),
        1,
        "the count stopped at the first text, it converted no other"
    );
    assert_eq!(Fired::Text.name(), "text");
}

/// The mutant without the per-text guard builds facts for that page.
#[test]
fn r8_1_the_mutant_without_the_per_text_guard_builds_facts_for_a_text_over_its_budget() {
    let base = one_text_over_budget();
    let setup = BaseFacts::compute_guarded(&loaded(&base), &allocation_guard());
    assert!(setup.facts.is_none() && setup.guard == Some(Fired::Text));
    let _fault = inject(Fault::TextGuardSkipped);
    let setup = BaseFacts::compute_guarded(&loaded(&base), &allocation_guard());
    assert!(setup.facts.is_some() && setup.guard.is_none());
}

/// The allocation companion is deterministic: the same page gives the same answer on every run, and
/// the budget sits between the largest honest text and the shapes the guard exists for.
#[test]
fn r8_1_the_allocation_budget_separates_honest_texts_from_nested_styles() {
    for (shape, label) in [
        (Honest::Plain, "plain"),
        (Honest::Edited(2_000), "2,000 edits"),
        (Honest::Formatted(500), "500 clusters"),
        (Honest::FormattedEdited(500, 2_000), "500 clusters and 2,000 edits"),
        (Honest::Reversed(25_000), "25,000 reverse-typed pieces"),
    ] {
        let (_, bytes) = text_cost(&honest_text_page(shape));
        assert!(
            bytes * 4 <= TEXT_ALLOCATION_LIMIT_BYTES,
            "{label}: {bytes} bytes is not far below the per-text budget of {TEXT_ALLOCATION_LIMIT_BYTES}"
        );
    }
    let (_, nested) = text_cost(&one_text_over_budget());
    assert!(nested > TEXT_ALLOCATION_LIMIT_BYTES, "{nested}");
    let (_, under) = text_cost(&anchor_page(50, 1, 800, Anchors::NestedSameKey));
    assert!(under < TEXT_ALLOCATION_LIMIT_BYTES, "{under}");
}

/// Honest pages at the documented limits keep their facts under the production guard (the real
/// clock and the production budgets): the largest block text the limits allow, plain, heavily
/// formatted, with a long edit history, and a page of ten of them (the document limit).
#[test]
fn r8_1_honest_pages_at_the_documented_limits_stay_incremental_with_the_production_budgets() {
    let mut pages: Vec<(String, Vec<u8>)> = [
        (Honest::Plain, "plain"),
        (Honest::Edited(2_000), "2,000 edits by two peers"),
        (Honest::Formatted(500), "500 clusters"),
        (Honest::FormattedEdited(500, 2_000), "500 clusters, 2,000 edits"),
        (Honest::Reversed(25_000), "25,000 reverse-typed pieces"),
    ]
    .into_iter()
    .map(|(shape, label)| (label.to_owned(), honest_text_page(shape)))
    .collect();
    pages.push(("ten full blocks (the document limit)".to_owned(), ten_full_blocks()));
    for (label, base) in &pages {
        let (items, budget) = items_and_budget(base);
        assert!(items <= budget, "{label}: {items} items over the budget of {budget}");
        let cost: SetupCost = setup_cost_under(base, SetupGuard::start);
        assert!(
            cost.has_facts && cost.fired.is_none(),
            "{label}: the production guard removed the facts ({:?})",
            cost.fired
        );
    }
}

/// Ten blocks of the largest text the limits allow: the document's text limit.
fn ten_full_blocks() -> Vec<u8> {
    let raw = LoroDoc::new();
    raw.import(&flat_page_of(10)).unwrap();
    raw.set_peer_id(48_950).unwrap();
    crate::page_inline::configure(&raw);
    for index in 0..10 {
        let text = first_text(&raw, index);
        let length = text.len_unicode();
        text.insert(
            length,
            &"abcdefghij".repeat(10_000)[..crate::limits::TEXT_BLOCK_CHARS_MAX - length],
        )
        .unwrap();
    }
    raw.commit();
    raw.export(ExportMode::snapshot()).unwrap()
}

/// 40 formatted texts, loaded and prewarmed, for the tests of the stepping clock.
fn forty_texts() -> Vec<u8> {
    super::window_round6_tests::formatted_page(40, super::window_round6_tests::Formatting::Clusters(1))
}

/// The facts under a per-text CPU budget of `budget_steps` reads of the stepping clock (1 ms a read)
/// and no other limit, and how often the clock was read.
fn text_cpu_guarded(engine: &LoroCollabEngine, budget_steps: u64) -> (crate::facts::SetupFacts, u64) {
    stepping_clock::install(1_000_000);
    let guard = SetupGuard::with_limits(u64::MAX, u64::MAX, stepping_clock::read)
        .with_text_limits(u64::MAX, budget_steps * 1_000_000);
    let setup = BaseFacts::compute_guarded(engine, &guard);
    (setup, stepping_clock::reads())
}

/// A text is measured by two reads of the clock after its start: after its decode (before its
/// conversion) and after its conversion. A budget of one step passes the first and fails the second;
/// a budget under one step fails the first, and the conversion is not read at all.
#[test]
fn r8_1_a_text_is_checked_after_its_decode_and_after_its_conversion() {
    let base = forty_texts();
    // Three reads per text (start, after the decode, after the conversion), and none for the page.
    let (setup, reads) = text_cpu_guarded(&loaded(&base), 1_000_000);
    assert!(setup.facts.is_some() && setup.guard.is_none());
    assert_eq!(reads, 3 * 40);
    // One step: passes after the decode (1 step), fails after the conversion (2 steps), so the
    // first text is the one that costs too much, and no other is counted.
    crate::walk_counter::reset();
    let (setup, reads) = text_cpu_guarded(&loaded(&base), 1);
    assert_eq!(setup.guard, Some(Fired::Text));
    assert!(setup.facts.is_none());
    assert_eq!(reads, 3);
    assert_eq!(crate::walk_counter::delta_reads().len(), 1);
    // Two steps pass: the text costs exactly two.
    assert!(text_cpu_guarded(&loaded(&base), 2).0.facts.is_some());
    // A text over its budget after the decode alone stops before the conversion: two reads.
    crate::walk_counter::reset();
    stepping_clock::install(1_000_000);
    let guard = SetupGuard::with_limits(u64::MAX, u64::MAX, stepping_clock::read).with_text_limits(u64::MAX, 500_000);
    let setup = BaseFacts::compute_guarded(&loaded(&base), &guard);
    assert_eq!(setup.guard, Some(Fired::Text));
    assert_eq!(stepping_clock::reads(), 2, "the conversion of the text was not started");
}

/// The text that costs too much is the one a long stretch of CPU time passes in, wherever it is on
/// the page: the texts before it are counted, the texts after it are not, and the page has no facts.
#[test]
fn r8_1_the_text_that_a_long_stretch_of_cpu_time_passes_in_is_the_one_that_fires() {
    let base = forty_texts();
    for slow in [0_u64, 7, 39] {
        crate::walk_counter::reset();
        stepping_clock::install(1_000_000);
        // Reads of text `slow` are 3 * slow + 1 (start), +2 (decode), +3 (conversion): the jump lands
        // on the read after its decode.
        stepping_clock::jump_at(3 * slow + 2, 1_000_000_000);
        let guard = SetupGuard::with_limits(u64::MAX, u64::MAX, stepping_clock::read)
            .with_text_limits(u64::MAX, TEXT_CPU_BUDGET_NANOS);
        let setup = BaseFacts::compute_guarded(&loaded(&base), &guard);
        assert_eq!(setup.guard, Some(Fired::Text), "slow text {slow}");
        assert!(setup.facts.is_none());
        let counted = crate::walk_counter::delta_reads().len();
        assert_eq!(
            counted,
            usize::try_from(slow).unwrap() + 1,
            "texts counted for slow text {slow}"
        );
    }
    // The same page with no jump has facts.
    stepping_clock::install(1_000_000);
    let guard = SetupGuard::with_limits(u64::MAX, u64::MAX, stepping_clock::read)
        .with_text_limits(u64::MAX, TEXT_CPU_BUDGET_NANOS);
    assert!(BaseFacts::compute_guarded(&loaded(&base), &guard).facts.is_some());
}

/// The per-text budget follows the window's CPU ceiling.
#[test]
fn r8_1_the_text_budget_is_the_ceiling_of_the_window() {
    assert_eq!(
        TEXT_CPU_BUDGET_NANOS,
        crate::isolation::DECODE_APPLY_CPU_MS_MAX * 1_000_000,
        "50 ms"
    );
    assert_eq!(TEXT_CPU_BUDGET_NANOS, 50_000_000);
}

/// A clock that works for its first read and not for the next ones.
fn dies_after_start() -> Option<u64> {
    if stepping_clock::reads() == 0 {
        stepping_clock::read()
    } else {
        stepping_clock::dead()
    }
}

/// A CPU clock that cannot be read fails closed: no facts, for the page budget and for a text.
#[test]
fn r8_5_a_cpu_clock_that_cannot_be_read_yields_no_facts() {
    let base = forty_texts();
    // Dead from the start of the guard.
    stepping_clock::install(1_000_000);
    let guard = SetupGuard::with_limits(u64::MAX, 1_000_000_000, stepping_clock::dead);
    let setup = BaseFacts::compute_guarded(&loaded(&base), &guard);
    assert!(setup.facts.is_none());
    assert_eq!(setup.guard, Some(Fired::Cpu));
    // Dead only for the per-text budget.
    stepping_clock::install(1_000_000);
    let guard = SetupGuard::with_limits(u64::MAX, u64::MAX, stepping_clock::dead)
        .with_text_limits(u64::MAX, TEXT_CPU_BUDGET_NANOS);
    let setup = BaseFacts::compute_guarded(&loaded(&base), &guard);
    assert!(setup.facts.is_none());
    assert_eq!(setup.guard, Some(Fired::Text));
    // A clock that dies after the guard started: the first read works, the next ones do not.
    stepping_clock::install(1_000_000);
    let guard = SetupGuard::with_limits(u64::MAX, 1_000_000_000, dies_after_start);
    assert_eq!(guard.fired(), Some(Fired::Cpu));
    // The real clock works on the platforms this runs on.
    assert!(thread_cpu_nanos().is_some());
}

/// The host's setting of the setup's CPU budget: a number inside the range, the default otherwise.
#[test]
fn r8_4_the_cpu_budget_setting_is_taken_inside_its_range_and_defaults_outside_it() {
    use crate::setup_guard::{SETUP_CPU_BUDGET_NANOS, cpu_budget_nanos_from_setting};
    assert_eq!(SETUP_CPU_BUDGET_NANOS, 2_500_000_000);
    assert_eq!(cpu_budget_nanos_from_setting(None), SETUP_CPU_BUDGET_NANOS);
    assert_eq!(cpu_budget_nanos_from_setting(Some("2500")), SETUP_CPU_BUDGET_NANOS);
    assert_eq!(cpu_budget_nanos_from_setting(Some("500")), 500_000_000);
    assert_eq!(cpu_budget_nanos_from_setting(Some("4000")), 4_000_000_000);
    assert_eq!(cpu_budget_nanos_from_setting(Some(" 3000 ")), 3_000_000_000);
    for bad in ["", "0", "499", "4001", "18446744073709551615", "-1", "1e3", "fast"] {
        assert_eq!(
            cpu_budget_nanos_from_setting(Some(bad)),
            SETUP_CPU_BUDGET_NANOS,
            "{bad:?}"
        );
    }
}

/// The review's growth scenario: one text is grown, window by window, with updates that the
/// incremental window accepts, until its setup cost passes the per-text budget; the guard then
/// removes the facts, and the next write is decided by the full window on a fresh load, where the
/// decode of the text is paid again inside the window (the worker reloads the engine after a
/// guard fired).
#[test]
fn r8_1_a_text_grown_through_accepted_windows_is_removed_from_the_facts_and_the_full_window_pays_its_decode() {
    const PER_WINDOW: usize = 50;
    let mut base = anchor_page(10, 1, 100, Anchors::NestedSameKey);
    let mut marks = 100;
    let mut windows = 0_u64;
    loop {
        let mut engine = loaded(&base);
        let setup = BaseFacts::compute_guarded(&engine, &allocation_guard());
        let Some(facts) = setup.facts else {
            assert_eq!(setup.guard, Some(Fired::Text));
            break;
        };
        let update = more_nested_marks(&base, 49_100 + windows, marks, PER_WINDOW);
        let accepted = crate::window_check::run_window(&mut engine, Some(&facts), &update, &mut |_| {})
            .expect("every window of the growth is accepted");
        assert!(accepted.report.is_some(), "and decided incrementally");
        base = engine.export_snapshot().unwrap();
        marks += PER_WINDOW;
        windows += 1;
        assert!(windows < 200, "the guard never removed the facts, {marks} marks");
    }
    println!("{windows} accepted windows of {PER_WINDOW} marks, the guard fired at {marks} marks");
    assert!(marks > 800 && marks < 1_800, "{marks}");
    // The next write, as the worker runs it after a fired guard: a fresh load, no facts.
    let update = more_nested_marks(&base, 49_999, marks, 2);
    let mut fresh = loaded(&base);
    let before = requested_bytes();
    let accepted = crate::window_check::run_window(&mut fresh, None, &update, &mut |_| {}).expect("accepted");
    let full_window_bytes = requested_bytes() - before;
    assert_eq!(
        accepted.fallback,
        Some(crate::window_check::FallbackReason::FactsAbsent)
    );
    assert!(
        full_window_bytes >= TEXT_ALLOCATION_LIMIT_BYTES,
        "the full window on a fresh load pays the text's decode: {full_window_bytes} bytes"
    );
    // On the engine the setup had decoded, the window would not have paid it.
    let (mut decoded, _) = decoded_page(&base);
    let before = requested_bytes();
    crate::window_check::run_window(&mut decoded, None, &update, &mut |_| {}).expect("accepted");
    assert!(
        requested_bytes() - before < TEXT_ALLOCATION_LIMIT_BYTES / 2,
        "the decoded engine pays only the conversion: that is why the worker reloads"
    );
}

/// Cases for the differential harness: a page with one hostile text, the per-text guard firing and
/// holding, in allocation and in CPU time. Every mutant that changes what the guard does is caught
/// by a path that differs from the worker's.
pub(super) fn round8_cases(scale: u64) -> Vec<Case> {
    let base = std::sync::Arc::new(anchor_page(60, 2, 300, Anchors::NestedSameKey));
    let mut cases = Vec::new();
    for k in 0..scale {
        let typed_hostile = super::hand_encoded::raw_edit(&base, 50_100 + k, |raw| {
            first_text(raw, 0).insert(0, "h").unwrap();
        });
        let typed_other = super::hand_encoded::raw_edit(&base, 50_200 + k, |raw| {
            first_text(raw, 9).insert(1, "o").unwrap();
        });
        let marked_other = super::hand_encoded::raw_edit(&base, 50_300 + k, |raw| {
            let text = first_text(raw, 11);
            text.insert(1, "mm").unwrap();
            text.mark(1..2, "bold", true).unwrap();
        });
        for (kind, update) in [
            ("text_typed_hostile", typed_hostile),
            ("text_typed_other", typed_other),
            ("text_marked_other", marked_other),
        ] {
            for (name, facts) in [
                ("fires", FactsFrom::TextGuarded(1024 * 1024)),
                ("holds", FactsFrom::TextGuarded(u64::MAX)),
                ("cpu_fires", FactsFrom::TextCpuStepped(1)),
                ("cpu_holds", FactsFrom::TextCpuStepped(1_000_000)),
            ] {
                cases.push(Case {
                    label: format!("round8/{kind}/{name}/{k}"),
                    family: "round8",
                    kind,
                    base: base.clone(),
                    update: update.clone(),
                    expect: None,
                    facts,
                });
            }
        }
    }
    cases
}
