//! The decision `window_check::run_window` reports through its `on_decision` callback, which the
//! worker writes to the host as its decision record (`isolation::wire::WindowDecision`): once, as
//! soon as the check has decided and before the full computation of a fallback runs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use super::hand_encoded::{accepted, raw_edit};
use super::window_round2_tests::flat_page_of;
use super::window_round4_tests::first_text;
use super::{CollabEngine, LoroCollabEngine};
use crate::editor_doc_bridge::{EditorInitialBlock, initialize_editor_doc};
use crate::facts::BaseFacts;
use crate::isolation::wire::WindowDecision;
use crate::walk_counter;
use crate::window_check::{FallbackReason, WindowAccepted, fault, run_window};
use crate::{CollabError, DocumentLimits};

/// What one window run reported: every decision it announced, with the window counters as they
/// stood at that moment, and the result.
struct Run {
    announced: Vec<(WindowDecision, walk_counter::WindowCounts)>,
    result: Result<WindowAccepted, CollabError>,
}

fn run(base: &[u8], update: &[u8], with_facts: bool) -> Run {
    let mut engine = LoroCollabEngine::load(base).unwrap();
    engine.prewarm_for_writes();
    let facts = if with_facts { BaseFacts::compute(&engine) } else { None };
    assert_eq!(with_facts, facts.is_some(), "the base has facts exactly when asked for");
    walk_counter::reset();
    let mut announced = Vec::new();
    let result = run_window(&mut engine, facts.as_ref(), update, &mut |decision| {
        announced.push((decision, walk_counter::window_counts()));
    });
    Run { announced, result }
}

fn type_into_first_block(base: &[u8], peer: u64, text: &'static str) -> Vec<u8> {
    raw_edit(base, peer, |raw| first_text(raw, 0).insert(0, text).unwrap())
}

#[test]
fn an_incremental_accept_is_announced_once() {
    let base = flat_page_of(5);
    let update = type_into_first_block(&base, 71, "typed");
    let run = run(&base, &update, true);
    assert!(run.result.unwrap().report.is_some());
    assert_eq!(
        run.announced.iter().map(|(decision, _)| *decision).collect::<Vec<_>>(),
        vec![WindowDecision::Incremental]
    );
}

#[test]
fn a_fallback_is_announced_once_before_the_full_computation_runs() {
    // No facts: the full computation decides.
    let base = flat_page_of(5);
    let update = type_into_first_block(&base, 72, "typed");
    let run = run(&base, &update, false);
    let accepted = run.result.unwrap();
    assert_eq!(accepted.fallback, Some(FallbackReason::FactsAbsent));
    let [(decision, counts)] = run.announced.as_slice() else {
        panic!(
            "announced {:?}",
            run.announced.iter().map(|(d, _)| d).collect::<Vec<_>>()
        );
    };
    assert_eq!(*decision, WindowDecision::Fallback(FallbackReason::FactsAbsent));
    // At the time of the announcement the full computation had not started: it records its fallback
    // and reads every block after the callback.
    assert_eq!(counts.fallbacks, 0, "announced before the fallback branch ran");
    let after = walk_counter::window_counts();
    assert_eq!(after.fallbacks, 1);
    assert!(
        after.block_reads > counts.block_reads,
        "the full computation read blocks after the announcement ({} -> {})",
        counts.block_reads,
        after.block_reads
    );
}

#[test]
fn an_update_concurrent_with_the_base_is_announced_as_not_sequential() {
    let base0 = flat_page_of(5);
    let first = type_into_first_block(&base0, 73, "one");
    let concurrent = type_into_first_block(&base0, 74, "two");
    let base1 = accepted(&base0, &first);
    let run = run(&base1, &concurrent, true);
    let accepted = run.result.unwrap();
    assert!(accepted.report.is_none(), "the full computation decided");
    assert_eq!(accepted.fallback, Some(FallbackReason::NotSequential));
    assert_eq!(
        run.announced.iter().map(|(decision, _)| *decision).collect::<Vec<_>>(),
        vec![WindowDecision::Fallback(FallbackReason::NotSequential)]
    );
    assert_eq!(run.announced[0].1.fallbacks, 0, "announced before the full computation");
}

#[test]
fn a_total_over_its_limit_is_announced_as_refused() {
    let limits = DocumentLimits::default();
    let blocks = 1_000;
    let chars_each = limits.document_text_chars_max / blocks;
    let mut engine = LoroCollabEngine::new_empty(75);
    let list: Vec<EditorInitialBlock> = (0..blocks)
        .map(|index| EditorInitialBlock {
            id: format!("b{index}"),
            kind: "paragraph".into(),
            text: "x".repeat(chars_each),
            indent: 0,
            level: None,
        })
        .collect();
    initialize_editor_doc(&engine.doc, &list).unwrap();
    engine.doc.commit();
    engine.invalidate_editor_model_floor();
    let base = engine.export_snapshot().unwrap();
    // The page holds exactly `document_text_chars_max` characters; one more is over the total.
    let update = type_into_first_block(&base, 76, "y");
    let run = run(&base, &update, true);
    let Err(CollabError::LimitExceeded { limit_kind, .. }) = run.result else {
        panic!("expected a refusal by the total");
    };
    assert_eq!(limit_kind, "document_text_chars");
    assert_eq!(
        run.announced.iter().map(|(decision, _)| *decision).collect::<Vec<_>>(),
        vec![WindowDecision::Refused]
    );
    assert_eq!(walk_counter::window_counts().direct_refusals, 1);
}

#[test]
fn an_import_that_is_refused_before_the_check_announces_nothing() {
    let base = flat_page_of(5);
    let run = run(&base, b"not an update", true);
    assert!(run.result.is_err());
    assert!(run.announced.is_empty(), "no check ran, so there is no decision");
}

/// The mutant "the verdict record is not written": a window that never announces its decision is
/// caught by the tests above (a worker that never tells the host would leave the host unable to
/// know that a killed attempt was not sequential).
#[test]
fn the_mutant_that_never_announces_its_decision_is_caught() {
    let base0 = flat_page_of(5);
    let first = type_into_first_block(&base0, 77, "one");
    let concurrent = type_into_first_block(&base0, 78, "two");
    let base1 = accepted(&base0, &first);

    let caught = |body: fn(&[u8], &[u8])| {
        let injected = fault::inject(fault::Fault::DecisionNotReported);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(&base1, &concurrent)));
        drop(injected);
        outcome.is_err()
    };
    let body: fn(&[u8], &[u8]) = |base, update| {
        let run = run(base, update, true);
        assert_eq!(
            run.announced.iter().map(|(decision, _)| *decision).collect::<Vec<_>>(),
            vec![WindowDecision::Fallback(FallbackReason::NotSequential)]
        );
    };
    assert!(
        caught(body),
        "a window that announces nothing must fail the decision tests"
    );
    // Without the mutant the same body passes.
    body(&base1, &concurrent);
}
