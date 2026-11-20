//! The number of full snapshots one batch takes must not depend on the batch length or on the
//! page size.
//!
//! The per-operation path took one snapshot per operation: up to 100 whole-page projections for
//! one command, 87 to 89% of the in-process time of a write at 5,000 blocks. The count is made
//! with the block-read counter (`walk_counter`): a snapshot of an editor page reads the block list
//! once, and nothing else on this path does, so the delta of the counter over a batch is the
//! number of snapshots the batch took. The oracle's count is asserted too: it proves the counter
//! sees a snapshot, so a regression to the old shape fails here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use super::operation_tracker_tests::Page;
use super::{
    DocumentLimits, OperationTracker, apply_operations_checked, apply_operations_checked_per_operation_snapshot,
    run_checked,
};
use crate::engine::LoroCollabEngine;
use crate::operation::{NodeId, NodeKind, Operation};
use crate::walk_counter;

/// What the REST `insert_block` and `semantic_patch` commands send: blocks, each with its text.
fn batch_of(length: usize) -> Vec<Operation> {
    let mut batch = Vec::new();
    while batch.len() < length {
        let id = NodeId::from(format!("new{}", batch.len()));
        batch.push(Operation::CreateNode {
            id: id.clone(),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        });
        if batch.len() < length {
            batch.push(Operation::InsertText {
                id,
                index: 0,
                text: "typed".into(),
            });
        }
    }
    batch
}

/// A page of `size` plain blocks at the editor floor of `page`, with no deletes or nesting.
fn plain_page(page: Page, size: usize) -> LoroCollabEngine {
    let mut engine = LoroCollabEngine::new_empty(7);
    engine.initialize_page_editor().unwrap();
    if page != Page::Flat {
        engine
            .initialize_page_structure_model(if page == Page::Structured4 { 4 } else { 5 })
            .unwrap();
    }
    for index in 0..size {
        engine
            .apply_content_operation(&Operation::CreateNode {
                id: NodeId::from(format!("s{index}")),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            })
            .unwrap();
    }
    engine
}

fn snapshots_of_new_path(base: &LoroCollabEngine, length: usize) -> (usize, usize) {
    let mut engine = base.fork().unwrap();
    let batch = batch_of(length);
    walk_counter::reset();
    let mut tracker = OperationTracker::new(None);
    let mut applied = 0;
    run_checked(
        &mut engine,
        &batch,
        &DocumentLimits::default(),
        &mut tracker,
        &mut applied,
    )
    .unwrap();
    assert_eq!(applied, length);
    (walk_counter::counts().1, tracker.views_built())
}

fn snapshots_of_oracle(base: &LoroCollabEngine, length: usize) -> usize {
    let mut engine = base.fork().unwrap();
    let batch = batch_of(length);
    walk_counter::reset();
    let (result, applied) =
        apply_operations_checked_per_operation_snapshot(&mut engine, &batch, &DocumentLimits::default());
    result.unwrap();
    assert_eq!(applied, length);
    walk_counter::counts().1
}

#[test]
fn a_batch_takes_one_snapshot_whatever_its_length_and_the_page_size() {
    for page in [Page::Flat, Page::Structured4, Page::Structured5] {
        for size in [200_usize, 2_000] {
            let base = plain_page(page, size);
            for length in [1_usize, 10, 100] {
                let (reads, views) = snapshots_of_new_path(&base, length);
                let oracle = snapshots_of_oracle(&base, length);
                println!(
                    "{page:?} {size} blocks, {length} operations: new {reads} snapshot(s), per-operation path {oracle}"
                );
                assert_eq!(
                    oracle, length,
                    "{page:?}: the counter must see one snapshot per operation of the old path"
                );
                assert_eq!(reads, 1, "{page:?} {size} blocks {length} operations: snapshots taken");
                assert_eq!(views, 1);
            }
        }
    }
}

#[test]
fn a_seeded_batch_takes_no_snapshot() {
    let mut engine = plain_page(Page::Flat, 200);
    let seed = engine.semantic_snapshot().unwrap();
    let batch = batch_of(100);
    walk_counter::reset();
    apply_operations_checked(&mut engine, &batch, &DocumentLimits::default(), Some(seed)).unwrap();
    assert_eq!(walk_counter::counts().1, 0);
}

/// Operations whose effect is not modelled make the next check read the engine again, which is
/// what the old path did for every operation: the cost is bounded by the old cost, never above.
#[test]
fn unmodelled_operations_cost_at_most_one_snapshot_each() {
    let mut engine = plain_page(Page::Flat, 200);
    let mut batch = Vec::new();
    for (index, id) in (0..10).map(|n| format!("s{n}")).enumerate() {
        batch.push(Operation::DeleteNode {
            id: NodeId::from(id.as_str()),
        });
        batch.push(Operation::CreateNode {
            id: NodeId::from(format!("again{index}")),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        });
    }
    walk_counter::reset();
    apply_operations_checked(&mut engine, &batch, &DocumentLimits::default(), None).unwrap();
    let reads = walk_counter::counts().1;
    assert!(reads <= batch.len(), "{reads} snapshots for {} operations", batch.len());
    println!(
        "10 deletes interleaved with 10 creates: {reads} snapshots for {} operations",
        batch.len()
    );
}

/// Wall-clock comparison, printed and never asserted. Run in release mode:
/// `cargo test -p collab-core --release --lib tracker_timing -- --ignored --nocapture`.
/// Method: one base page per (kind, size); each run forks the base, applies the same 100-operation
/// batch (50 blocks with their text) through the path measured, and takes the elapsed time of
/// that call; the median of `TIMING_RUNS` runs (default 5) per cell, the two paths interleaved.
#[test]
#[ignore = "timing measurement, not an assertion"]
fn tracker_timing_old_against_new() {
    use std::time::Instant;
    let runs: usize = std::env::var("TIMING_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let only = std::env::var("TIMING_PAGE").unwrap_or_default();
    let median = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values[values.len() / 2]
    };
    let batch = batch_of(100);
    let limits = DocumentLimits::default();
    for page in [Page::Flat, Page::Structured4] {
        if !only.is_empty() && !format!("{page:?}").starts_with(&only) {
            continue;
        }
        for size in [1_000_usize, 2_700, 5_000] {
            let base = plain_page(page, size);
            let (mut old, mut new) = (Vec::new(), Vec::new());
            for _ in 0..runs {
                let mut engine = base.fork().unwrap();
                let start = Instant::now();
                apply_operations_checked_per_operation_snapshot(&mut engine, &batch, &limits)
                    .0
                    .unwrap();
                old.push(start.elapsed().as_secs_f64() * 1e3);
                let mut engine = base.fork().unwrap();
                let start = Instant::now();
                apply_operations_checked(&mut engine, &batch, &limits, None).unwrap();
                new.push(start.elapsed().as_secs_f64() * 1e3);
            }
            println!(
                "TIMING {page:?} {size} blocks: old {:.1} ms, new {:.1} ms",
                median(old),
                median(new)
            );
        }
    }
}
