//! `prewarm_for_writes` is setup work for the isolated worker: it must change nothing a reader or
//! a decision can see, and it must cover every container the import of an ordinary update would
//! otherwise decode inside the metered window.
//!
//! The cost is counted, not timed: the bytes the import allocates on the calling thread measure
//! the work it did. One update per container kind touches that container and nothing else, so a
//! prewarm that stops covering one container fails the pin of exactly that container.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use loro::{ExportMode, LoroDoc, LoroMap};

use super::floor_equivalence_tests::{children, page, raw_update};
use super::read_side_effect_tests::{assert_unchanged, documents};
use super::window_differential_tests::{PrewarmedFull, difference_with, observe};
use super::window_fixtures::{Case, boundary_cases, generated_cases, hostile_cases, run_parallel};
use super::{CollabEngine, LoroCollabEngine, PrewarmParts};
use crate::operation::{NodeId, NodeKind, Operation};
use crate::reserved_roots::{ALIASES, ORDER, TREE};
use crate::test_alloc;

const SMALL_PAGE: usize = 200;
const LARGE_PAGE: usize = 2_000;
/// What an import may allocate beyond the same import on a ten times smaller page: handles and
/// the update's own ops, nothing that scales with the page.
const SLACK_BYTES: usize = 16 * 1024;

/// One kind of container an update can touch, with the page it needs and the raw edit that
/// changes that container only.
#[derive(Debug, Clone, Copy)]
struct Touch {
    name: &'static str,
    floor: u32,
    edit: fn(&LoroDoc),
    only: PrewarmParts,
}

fn touches() -> Vec<Touch> {
    vec![
        Touch {
            name: "root block list (doc.children)",
            floor: 1,
            only: PrewarmParts::BLOCK_LIST,
            edit: |doc| {
                children(doc)
                    .insert_container(0, LoroMap::new())
                    .unwrap()
                    .insert("nodeName", "paragraph")
                    .unwrap();
            },
        },
        Touch {
            name: "alias map",
            floor: 1,
            only: PrewarmParts::ALIASES,
            edit: |doc| doc.get_map(ALIASES).insert("fresh", "cid:1@1:Map").unwrap(),
        },
        Touch {
            name: "navigator order map",
            floor: 1,
            only: PrewarmParts::ORDER,
            edit: |doc| doc.get_map(ORDER).insert("fresh", "m").unwrap(),
        },
        Touch {
            name: "placement tree",
            floor: 4,
            only: PrewarmParts::STRUCTURE_TREE,
            edit: |doc| {
                let _ = doc.get_tree(TREE).create(None).unwrap();
            },
        },
    ]
}

/// A base page with `blocks` blocks and aliases, an order entry, and (from floor 4) a placement
/// tree, as snapshot bytes. Every container the touches name is non-empty.
fn base_page(floor: u32, blocks: usize) -> (LoroCollabEngine, Vec<u8>) {
    let mut engine = page(floor, blocks, 91);
    for index in 0..blocks {
        engine.set_navigator_order(&format!("nav{index}"), "a").unwrap();
    }
    let snapshot = engine.export_snapshot().unwrap();
    (engine, snapshot)
}

/// How much of the base is materialised before the import.
#[derive(Debug, Clone, Copy)]
enum Warm {
    /// Nothing: the import builds everything it needs.
    Cold,
    /// Exactly these containers (the root index is always built).
    Parts(PrewarmParts),
    /// `prewarm_for_writes`, what the worker calls.
    Full,
}

/// Bytes `import_client_update` allocates for `touch` on a freshly loaded base.
fn import_allocation(touch: Touch, blocks: usize, warm: Warm) -> usize {
    let (author, snapshot) = base_page(touch.floor, blocks);
    let update = raw_update(&author, 4242, touch.edit);
    let mut worker = LoroCollabEngine::load(&snapshot).unwrap();
    match warm {
        Warm::Cold => {}
        Warm::Parts(parts) => worker.prewarm_parts(parts),
        Warm::Full => worker.prewarm_for_writes(),
    }
    let before = test_alloc::allocated_bytes();
    worker.import_client_update(&update).unwrap();
    test_alloc::allocated_bytes().saturating_sub(before)
}

/// The growth of the import's allocation from the small to the large page.
fn growth(touch: Touch, warm: Warm) -> (usize, usize) {
    let small = import_allocation(touch, SMALL_PAGE, warm);
    let large = import_allocation(touch, LARGE_PAGE, warm);
    (small, large)
}

/// For every container an ordinary update touches: the read that materialises it removes the
/// growth of the import with the page, the root index alone does not, and `prewarm_for_writes`
/// does what that read alone does. A container dropped from `prewarm_for_writes` fails the last
/// check for its update; a read that no longer forces the decode fails the second.
#[test]
fn prewarm_covers_every_container_an_ordinary_update_touches() {
    for touch in touches() {
        let cold = growth(touch, Warm::Cold);
        let index_only = growth(touch, Warm::Parts(PrewarmParts::NONE));
        let own_read = growth(touch, Warm::Parts(touch.only));
        let full = growth(touch, Warm::Full);
        println!(
            "{:<32} cold {:>8} -> {:>9}  index only {:>8} -> {:>9}  own read {:>8} -> {:>9}  full {:>8} -> {:>9}",
            touch.name, cold.0, cold.1, index_only.0, index_only.1, own_read.0, own_read.1, full.0, full.1
        );
        let grow = |pair: (usize, usize)| pair.1.saturating_sub(pair.0);
        // The probe is wired: a cold import grows with the page.
        assert!(
            grow(cold) > 8 * SLACK_BYTES,
            "{}: the cold import no longer grows with the page: the probe measures nothing",
            touch.name
        );
        // The root index is part of the prewarm: it removes most of the cold import.
        assert!(
            grow(cold) > grow(index_only) + 8 * SLACK_BYTES,
            "{}: the root index no longer changes the cost of the import",
            touch.name
        );
        // The root index is not enough for this container.
        assert!(
            grow(index_only) > grow(own_read) + 2 * SLACK_BYTES,
            "{}: the read of this container no longer changes the cost of its update",
            touch.name
        );
        // The prewarm the worker runs does what the container's own read does.
        assert!(
            grow(full) <= grow(own_read) + SLACK_BYTES,
            "{}: prewarm_for_writes no longer covers this container ({} against {} bytes of growth)",
            touch.name,
            grow(full),
            grow(own_read)
        );
    }
}

/// A block insert authored by the engine itself (the shape of an ordinary editor write: a block
/// map in the root list, an alias entry and, on structured pages, a placement node) imports in
/// constant allocation after the prewarm, on every editor model.
#[test]
fn prewarm_makes_an_engine_authored_block_insert_independent_of_the_page() {
    let insert = |floor: u32, blocks: usize, prewarm: bool| {
        let (mut author, snapshot) = base_page(floor, blocks);
        let from = author.frontier();
        for id in ["fresh-a", "fresh-b"] {
            author
                .apply_content_operation(&Operation::CreateNode {
                    id: NodeId::from(id),
                    parent: None,
                    index: 0,
                    kind: NodeKind::Block,
                })
                .unwrap();
            author
                .apply_content_operation(&Operation::InsertText {
                    id: NodeId::from(id),
                    index: 0,
                    text: "typed".into(),
                })
                .unwrap();
        }
        let update = author.export_from(&from).unwrap();
        let mut worker = LoroCollabEngine::load(&snapshot).unwrap();
        if prewarm {
            worker.prewarm_for_writes();
        }
        let before = test_alloc::allocated_bytes();
        worker.import_client_update(&update).unwrap();
        test_alloc::allocated_bytes().saturating_sub(before)
    };
    for floor in [1_u32, 4, 5] {
        let cold = insert(floor, LARGE_PAGE, false);
        let warm_small = insert(floor, SMALL_PAGE, true);
        let warm_large = insert(floor, LARGE_PAGE, true);
        println!("floor {floor}: cold {cold}, prewarmed {warm_small} -> {warm_large}");
        assert!(
            warm_large * 8 < cold,
            "floor {floor}: the prewarm removed too little of the import ({warm_large} of {cold} bytes)"
        );
    }
}

/// The prewarm registers no root, changes no value and no export, on every document of the
/// read-side-effect corpus (documents without roots, with shadow roots, legacy pages, ...).
#[test]
fn prewarm_registers_nothing_and_changes_no_export() {
    for (label, doc) in documents() {
        let snapshot = doc.export(ExportMode::Snapshot).unwrap();
        let Ok(engine) = LoroCollabEngine::load(&snapshot) else {
            continue;
        };
        assert_unchanged(&label, &engine.doc, || engine.prewarm_for_writes());
    }
    for floor in [1_u32, 4, 5] {
        let (engine, _) = base_page(floor, 20);
        assert_unchanged(&format!("floor {floor} page"), &engine.doc, || {
            engine.prewarm_for_writes();
        });
    }
}

/// The prewarm is idempotent and a pure function of the document: a second call changes nothing,
/// and the engine reads the same afterwards.
#[test]
fn prewarm_is_idempotent_and_does_not_change_reads() {
    for floor in [1_u32, 4, 5] {
        let (engine, snapshot) = base_page(floor, 30);
        let cold = LoroCollabEngine::load(&snapshot).unwrap();
        engine.prewarm_for_writes();
        engine.prewarm_for_writes();
        assert_eq!(
            engine.semantic_snapshot().unwrap().semantic_hash(),
            cold.semantic_snapshot().unwrap().semantic_hash()
        );
        assert_eq!(
            engine.minimum_editor_model().unwrap(),
            cold.minimum_editor_model().unwrap()
        );
        assert_eq!(engine.frontier(), cold.frontier());
        assert_eq!(
            engine.export_snapshot().unwrap(),
            cold.export_snapshot().unwrap(),
            "floor {floor}"
        );
    }
}

/// Same decision (the whole error), same frontier, same exported value, same roots and same
/// projection: the prewarmed window against the plain one on every generated, boundary and hostile
/// case. The exported snapshot bytes are compared too, and a case in which they differ must be one
/// in which the document is equal in every other output: a Loro snapshot encodes a container from
/// its decoded state when that state exists and copies the stored bytes when it does not, so
/// forcing the root list's decode changes the encoding of an update that leaves that list alone.
/// The worker's snapshot bytes are only loaded by the host, never stored.
#[test]
fn prewarm_changes_no_decision_and_no_output() {
    let mut cases: Vec<Case> = generated_cases(1);
    cases.extend(boundary_cases());
    cases.extend(hostile_cases());
    let refs: Vec<&Case> = cases.iter().collect();
    let results = run_parallel(&refs, &|_, case| {
        let plain = observe(&super::window_differential_tests::FullComputation, case);
        let warm = observe(&PrewarmedFull, case);
        (
            difference_with(&plain, &warm, false),
            difference_with(&plain, &warm, true),
            plain.same_snapshot_bytes(&warm),
        )
    });
    let mut byte_differences = 0;
    for (case, (semantic, exact, bytes)) in cases.iter().zip(&results) {
        assert!(semantic.is_none(), "{}: {}", case.label, semantic.clone().unwrap());
        if exact.is_some() {
            assert_eq!(*bytes, Some(false), "{}: {exact:?}", case.label);
            byte_differences += 1;
        }
    }
    println!(
        "prewarm: {} cases, no difference in decision or output; the snapshot encoding differs in {byte_differences}",
        cases.len()
    );
}
