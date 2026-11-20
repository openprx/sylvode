//! Test-only counters for full-document walks on the Flow write path.
//!
//! The isolated-apply worker's CPU ceiling (`decode_apply_cpu_ms_max`) is spent mostly on walks
//! over every editor block. Wall-clock assertions on shared CI runners are flaky, so the
//! regression guards in this crate count walks instead: a change that adds a walk per import (or
//! per replayed tail update) fails deterministically. Counters are per thread, so tests running
//! in parallel do not see each other's walks.
use std::cell::{Cell, RefCell};

thread_local! {
    static FLOOR_WALKS: Cell<usize> = const { Cell::new(0) };
    static BLOCK_READS: Cell<usize> = const { Cell::new(0) };
    static SCANNED_OPS: Cell<usize> = const { Cell::new(0) };
    static HISTORICAL_ATOM_SCANS: Cell<usize> = const { Cell::new(0) };
    static NARROW_ATOM_SCANS: Cell<usize> = const { Cell::new(0) };
    static ROOT_PROBE_LOG: RefCell<Vec<(String, usize)>> = const { RefCell::new(Vec::new()) };
    static ROOT_FORKS: Cell<usize> = const { Cell::new(0) };
}

/// One evaluation of the editor-model floor (`minimum_editor_model` without a cache hit).
pub fn record_floor_walk() {
    FLOOR_WALKS.with(|count| count.set(count.get() + 1));
}

/// One pass over the editor document's block list (`editor_blocks`/`legacy_editor_blocks`).
pub fn record_block_read() {
    BLOCK_READS.with(|count| count.set(count.get() + 1));
}

/// One imported op visited by the downgrade guard's removal scan.
pub fn record_scanned_op() {
    SCANNED_OPS.with(|count| count.set(count.get() + 1));
}

/// One checkout-based read of the pre-import physical-atom signal (the guard's exact fallback).
pub fn record_historical_atom_scan() {
    HISTORICAL_ATOM_SCANS.with(|count| count.set(count.get() + 1));
}

/// One narrow scan of `doc.children` for atom block maps run by the downgrade guard.
pub fn record_narrow_atom_scan() {
    NARROW_ATOM_SCANS.with(|count| count.set(count.get() + 1));
}

/// One existence probe of a root container (`editor_structure::root_container`): the root name
/// and the bytes the probe allocated on this thread (a deterministic measure of the work it did).
/// A probe must cost the named root only; none may value the other roots.
pub fn record_root_probe(name: &str, allocated_bytes: usize) {
    ROOT_PROBE_LOG.with(|log| log.borrow_mut().push((name.to_owned(), allocated_bytes)));
}

/// One fork-and-compare run by a root probe (O(document); only a same-named root of another type
/// causes it).
pub fn record_root_fork() {
    ROOT_FORKS.with(|count| count.set(count.get() + 1));
}

/// Root probes recorded since the last [`reset`], in order: `(root name, bytes allocated)`.
pub fn root_probes() -> Vec<(String, usize)> {
    ROOT_PROBE_LOG.with(|log| log.borrow().clone())
}

/// Fork-and-compares run by root probes since the last [`reset`].
pub fn root_forks() -> usize {
    ROOT_FORKS.with(Cell::get)
}

/// Guard work recorded since the last [`reset`]:
/// `(imported ops scanned, narrow atom scans, historical atom scans)`.
pub fn guard_counts() -> (usize, usize, usize) {
    (
        SCANNED_OPS.with(Cell::get),
        NARROW_ATOM_SCANS.with(Cell::get),
        HISTORICAL_ATOM_SCANS.with(Cell::get),
    )
}

/// Walks recorded on this thread since the last [`reset`]: `(floor walks, block reads)`.
pub fn counts() -> (usize, usize) {
    (FLOOR_WALKS.with(Cell::get), BLOCK_READS.with(Cell::get))
}

pub fn reset() {
    FLOOR_WALKS.with(|count| count.set(0));
    BLOCK_READS.with(|count| count.set(0));
    SCANNED_OPS.with(|count| count.set(0));
    HISTORICAL_ATOM_SCANS.with(|count| count.set(0));
    NARROW_ATOM_SCANS.with(|count| count.set(0));
    ROOT_PROBE_LOG.with(|log| log.borrow_mut().clear());
    ROOT_FORKS.with(|count| count.set(0));
}
