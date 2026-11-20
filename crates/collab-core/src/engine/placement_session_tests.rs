//! The editor session's placement index against the recompute-every-time reference.
//!
//! Every structural and text edit of a command build used to rebuild the placement index of the
//! whole page. The session keeps one index per engine and updates it by each edit
//! (`editor_structure::PlacementIndex`). The reference path is still compiled under `cfg(test)`:
//! an engine with `recompute_placements` set runs the previous code verbatim. The differential
//! test below applies generated edit sequences to two engines loaded from the same bytes, one per
//! path, and requires:
//!
//! - the same result (or the same typed error) for every step;
//! - after every step, the incremental index equals an index built from the document;
//! - the same frontier after every step and, at the end, byte-identical exported updates and
//!   snapshots and identical semantic snapshots.
//!
//! The counting test pins the cost: the full placement-tree walks of one command build do not
//! depend on the number of edits or on the page size.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use std::collections::{BTreeMap, BTreeSet};

use super::floor_equivalence_tests::image_attrs;
use super::{CollabEngine, LoroCollabEngine};
use crate::editor_structure::{self, PlacementIndex};
use crate::operation::{NodeId, NodeKind, Operation};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % (n.max(1) as u64)).unwrap()
    }
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

const PEER: u64 = 4_242;

fn create(id: &str, parent: Option<&str>, index: u32) -> Operation {
    Operation::CreateNode {
        id: NodeId::from(id),
        parent: parent.map(NodeId::from),
        index,
        kind: NodeKind::Block,
    }
}

fn text(id: &str, value: &str) -> Operation {
    Operation::InsertText {
        id: NodeId::from(id),
        index: 0,
        text: value.into(),
    }
}

/// A structured page (model 4 or 5) with `extra` blocks created by random placements, moves and
/// deletes through the session path, so its history holds nested, moved and deleted blocks.
fn base_page(seed: u64, model: u32, extra: usize) -> Vec<u8> {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut engine = LoroCollabEngine::new_empty(seed);
    engine.initialize_page_editor().unwrap();
    engine.initialize_page_structure_model(model).unwrap();
    if model == 5 {
        engine
            .insert_page_atom("seed-image", None, 0, "image", &image_attrs())
            .unwrap();
    }
    // (alias, depth, live)
    let mut nodes: Vec<(String, u32, bool)> = Vec::new();
    let mut children: BTreeMap<Option<usize>, usize> = BTreeMap::new();
    let roots_at_start = engine.session_roots();
    children.insert(None, roots_at_start);
    for n in 0..extra {
        let live: Vec<usize> = (0..nodes.len()).filter(|&i| nodes[i].2).collect();
        let roll = rng.below(100);
        if roll < 8 && live.len() > 1 {
            let i = live[rng.below(live.len())];
            if engine
                .apply_content_operation(&Operation::DeleteNode {
                    id: NodeId::from(nodes[i].0.as_str()),
                })
                .is_ok()
            {
                nodes[i].2 = false;
            }
            continue;
        }
        if roll < 18 && live.len() > 1 {
            let i = live[rng.below(live.len())];
            let target = if rng.chance(30) {
                None
            } else {
                Some(live[rng.below(live.len())])
            };
            let count = children.get(&target).copied().unwrap_or(0);
            let index = u32::try_from(rng.below(count + 1)).unwrap();
            let parent = target.map(|t| NodeId::from(nodes[t].0.as_str()));
            if engine
                .apply_content_operation(&Operation::MoveNode {
                    id: NodeId::from(nodes[i].0.as_str()),
                    new_parent: parent,
                    index,
                })
                .is_ok()
            {
                nodes[i].1 = target.map_or(0, |t| nodes[t].1 + 1);
            }
            continue;
        }
        // Create: nest under a random live block (deep chains are common), else at the root.
        let parent = if !live.is_empty() && rng.chance(60) {
            let candidate = live[rng.below(live.len())];
            let deepest = live.iter().copied().max_by_key(|&i| nodes[i].1).unwrap_or(candidate);
            let pick = if rng.chance(25) { deepest } else { candidate };
            (nodes[pick].1 < 7).then_some(pick)
        } else {
            None
        };
        let count = children.get(&parent).copied().unwrap_or(0);
        let index = if rng.chance(50) { count } else { rng.below(count + 1) };
        let alias = format!("b{n}");
        if engine
            .apply_content_operation(&create(
                &alias,
                parent.map(|p| nodes[p].0.as_str()),
                u32::try_from(index).unwrap(),
            ))
            .is_ok()
        {
            nodes.push((alias, parent.map_or(0, |p| nodes[p].1 + 1), true));
            *children.entry(parent).or_default() += 1;
        }
    }
    engine.export_snapshot().unwrap()
}

impl LoroCollabEngine {
    /// Number of root-level active placements (test helper).
    fn session_roots(&self) -> usize {
        editor_structure::placements(&self.doc)
            .unwrap()
            .unwrap()
            .iter()
            .filter(|entry| entry.parent.is_none())
            .count()
    }
}

fn loaded(base: &[u8], recompute: bool) -> LoroCollabEngine {
    let mut engine = LoroCollabEngine::load(base).unwrap();
    engine.doc.set_peer_id(PEER).unwrap();
    engine.recompute_placements = recompute;
    engine
}

#[derive(Default)]
struct Coverage(BTreeMap<String, usize>);

impl Coverage {
    fn note(&mut self, key: String) {
        *self.0.entry(key).or_default() += 1;
    }
}

fn kind_of(op: &Operation) -> &'static str {
    match op {
        Operation::CreateNode { .. } => "create",
        Operation::MoveNode { .. } => "move",
        Operation::DeleteNode { .. } => "delete",
        Operation::InsertText { .. } => "text",
        Operation::SetProperty { .. } => "property",
        Operation::DeleteText { .. } => "delete_text",
    }
}

/// One generated edit against the page as it is now (`fresh` is the active placements in
/// document order). `dead` holds CIDs that lost their placement.
fn generate(rng: &mut Rng, fresh: &[editor_structure::Placement], dead: &[String], serial: &mut usize) -> Operation {
    let live_cid = |rng: &mut Rng| fresh[rng.below(fresh.len())].cid.clone();
    let any_cid = |rng: &mut Rng| {
        if !dead.is_empty() && rng.chance(8) {
            dead[rng.below(dead.len())].clone()
        } else if rng.chance(3) {
            "no-such-block".to_owned()
        } else {
            live_cid(rng)
        }
    };
    let children_of = |parent: Option<&str>| fresh.iter().filter(|e| e.parent.as_deref() == parent).count();
    let pick_parent = |rng: &mut Rng| -> Option<String> {
        if rng.chance(35) {
            None
        } else if rng.chance(20) {
            // The deepest block: the parent that hits the depth limit.
            fresh.iter().max_by_key(|e| e.depth).map(|e| e.cid.clone())
        } else {
            Some(any_cid(rng))
        }
    };
    let index_for = |rng: &mut Rng, parent: Option<&str>| -> u32 {
        let count = children_of(parent);
        let index = match rng.below(10) {
            0..=2 => 0,
            3..=5 => count,
            6 => count / 2,
            7 => count + 1,
            _ => rng.below(count + 1),
        };
        u32::try_from(index).unwrap()
    };
    match rng.below(100) {
        0..=29 => {
            let parent = pick_parent(rng);
            *serial += 1;
            let id = if rng.chance(3) {
                "b0".to_owned()
            } else {
                format!("g{serial}")
            };
            create(&id, parent.as_deref(), index_for(rng, parent.as_deref()))
        }
        30..=51 => {
            let id = any_cid(rng);
            let entry = fresh.iter().find(|e| e.cid == id);
            if let (Some(entry), true) = (entry, rng.chance(15)) {
                // Same parent, same position.
                return Operation::MoveNode {
                    id: NodeId::from(id.as_str()),
                    new_parent: entry.parent.as_deref().map(NodeId::from),
                    index: u32::try_from(entry.position).unwrap(),
                };
            }
            let parent = pick_parent(rng);
            Operation::MoveNode {
                id: NodeId::from(id.as_str()),
                new_parent: parent.as_deref().map(NodeId::from),
                index: index_for(rng, parent.as_deref()),
            }
        }
        52..=69 => Operation::DeleteNode {
            id: NodeId::from(any_cid(rng).as_str()),
        },
        70..=89 => text(&any_cid(rng), "t"),
        90..=95 => Operation::SetProperty {
            id: NodeId::from(any_cid(rng).as_str()),
            key: "block_type".into(),
            value: "heading".into(),
        },
        _ => {
            *serial += 1;
            create(&format!("g{serial}"), None, 0)
        }
    }
}

fn session_tree(engine: &LoroCollabEngine) -> loro::LoroTree {
    match editor_structure::reserved_root(&engine.doc, editor_structure::TREE).unwrap() {
        Some(loro::Container::Tree(tree)) => tree,
        _ => panic!("no editor tree"),
    }
}

fn fresh_placements(engine: &LoroCollabEngine) -> Vec<editor_structure::Placement> {
    editor_structure::placements(&engine.doc).unwrap().unwrap()
}

fn tuples(entries: &[editor_structure::Placement]) -> Vec<(String, Option<String>, u32, String, usize)> {
    entries
        .iter()
        .map(|e| {
            (
                e.cid.clone(),
                e.parent.clone(),
                e.depth,
                format!("{:?}", e.tree_id),
                e.position,
            )
        })
        .collect()
}

struct CaseReport {
    steps: usize,
    index_checks: usize,
}

/// One case: both engines load `base`, apply `steps` generated edits, and are compared.
fn run_case(base: &[u8], seed: u64, steps: usize, label: &str, coverage: &mut Coverage) -> CaseReport {
    let mut rng = Rng(seed.wrapping_mul(0xD134_2543_DE82_EF95) | 1);
    let mut oracle = loaded(base, true);
    let mut session = loaded(base, false);
    let from = session.frontier();
    let mut dead: Vec<String> = Vec::new();
    let mut serial = 0usize;
    let mut index_checks = 0usize;
    let large = fresh_placements(&session).len() > 400;
    for step in 0..steps {
        let fresh = fresh_placements(&oracle);
        let op = generate(&mut rng, &fresh, &dead, &mut serial);
        let expected = oracle.apply_content_operation(&op);
        let actual = session.apply_content_operation(&op);
        let context = format!("{label} step {step}: {op:?}");
        assert_eq!(
            format!("{expected:?}"),
            format!("{actual:?}"),
            "{context}: result differs"
        );
        coverage.note(format!(
            "{}:{}",
            kind_of(&op),
            match &actual {
                Ok(()) => "ok".to_owned(),
                Err(error) => format!("{error:?}")
                    .split('"')
                    .nth(1)
                    .map_or_else(|| "error".to_owned(), str::to_owned),
            }
        ));
        assert_eq!(oracle.frontier(), session.frontier(), "{context}: frontier differs");
        let after_oracle = fresh_placements(&oracle);
        let after_session = fresh_placements(&session);
        assert_eq!(
            tuples(&after_oracle),
            tuples(&after_session),
            "{context}: placements differ"
        );
        if let Some(index) = &session.session_index {
            assert!(!index.is_stale(), "{context}: stale index kept");
            assert_eq!(
                index,
                &PlacementIndex::from_placements(session_tree(&session), &after_session),
                "{context}: incremental index differs from a fresh build"
            );
            index_checks += 1;
        } else if actual.is_ok() && matches!(op, Operation::CreateNode { .. } | Operation::DeleteNode { .. }) {
            // A successful structural edit on a structured page leaves an index behind.
            panic!("{context}: no index after a successful structural edit");
        }
        if actual.is_ok()
            && let Operation::DeleteNode { id } = &op
        {
            dead.push(id.to_string());
        }
        let before: BTreeSet<_> = fresh.iter().map(|e| e.cid.clone()).collect();
        let after: BTreeSet<_> = after_session.iter().map(|e| e.cid.clone()).collect();
        dead.extend(before.difference(&after).cloned());
        if !large || step % 8 == 0 {
            assert_eq!(
                oracle.export_snapshot().unwrap(),
                session.export_snapshot().unwrap(),
                "{context}: snapshot bytes differ"
            );
        }
    }
    assert_eq!(
        oracle.export_from(&from).unwrap(),
        session.export_from(&from).unwrap(),
        "{label}: exported update bytes differ"
    );
    assert_eq!(
        oracle.export_snapshot().unwrap(),
        session.export_snapshot().unwrap(),
        "{label}: final snapshot bytes differ"
    );
    assert_eq!(
        oracle.semantic_snapshot().unwrap(),
        session.semantic_snapshot().unwrap(),
        "{label}: semantic snapshots differ"
    );
    CaseReport { steps, index_checks }
}

#[test]
fn session_index_path_equals_recompute_path_on_generated_pages_and_edits() {
    // (blocks added to the placeholder, seeds, steps)
    let plan: [(usize, u64, usize); 9] = [
        (0, 12, 60),
        (1, 12, 60),
        (2, 12, 60),
        (5, 12, 60),
        (20, 12, 60),
        (100, 12, 60),
        (400, 4, 50),
        (1_500, 2, 40),
        (5_000, 1, 40),
    ];
    let mut coverage = Coverage::default();
    let (mut cases, mut steps, mut checks) = (0, 0, 0);
    for model in [4_u32, 5] {
        for (extra, seeds, length) in plan {
            for seed in 1..=seeds {
                let base_seed = seed * 1_000 + extra as u64 + u64::from(model);
                let base = base_page(base_seed, model, extra);
                let label = format!("model {model} extra {extra} seed {base_seed}");
                let report = run_case(&base, base_seed, length, &label, &mut coverage);
                cases += 1;
                steps += report.steps;
                checks += report.index_checks;
            }
        }
    }
    println!("cases {cases}, steps {steps}, incremental-vs-fresh index comparisons {checks}");
    for (key, count) in &coverage.0 {
        println!("{key}: {count}");
    }
    for required in [
        "create:ok",
        "move:ok",
        "delete:ok",
        "text:ok",
        "property:ok",
        "create:editor_structure_depth_limit",
        "move:editor_structure_depth_limit",
        "move:editor_structure_cycle",
        "move:editor_structure_parent_not_found",
        "create:editor_structure_parent_not_found",
        "create:editor_block_index_out_of_bounds",
        "delete:editor_doc_requires_one_block",
        "delete:editor_block_not_found",
    ] {
        assert!(
            coverage.0.contains_key(required),
            "the generator never produced {required}"
        );
    }
}

/// Edits applied after something other than the session changed the document must not use the
/// index: an import (here one that moves and deletes blocks) retires it.
#[test]
fn a_change_the_session_did_not_make_retires_the_index() {
    let base = base_page(7, 4, 30);
    let mut oracle = loaded(&base, true);
    let mut session = loaded(&base, false);
    let fresh = fresh_placements(&oracle);
    // A second replica edits the same page and its update is imported by both engines.
    let mut remote = LoroCollabEngine::load(&base).unwrap();
    remote.doc.set_peer_id(PEER + 1).unwrap();
    let from = remote.frontier();
    let first = fresh[0].cid.clone();
    let last = fresh[fresh.len() - 1].cid.clone();
    remote
        .apply_content_operation(&Operation::DeleteNode {
            id: NodeId::from(last.as_str()),
        })
        .unwrap();
    remote
        .apply_content_operation(&Operation::MoveNode {
            id: NodeId::from(fresh[1].cid.as_str()),
            new_parent: None,
            index: 0,
        })
        .unwrap();
    let update = remote.export_from(&from).unwrap();
    // Warm the index on the session engine first.
    for engine in [&mut oracle, &mut session] {
        engine.apply_content_operation(&create("warm", None, 0)).unwrap();
    }
    assert!(session.session_index.is_some());
    for engine in [&mut oracle, &mut session] {
        engine.import_update(&update).unwrap();
    }
    assert!(session.session_index.is_none(), "an import must drop the index");
    for op in [
        Operation::DeleteNode {
            id: NodeId::from(last.as_str()),
        },
        create("after-import", Some(&first), 0),
        Operation::MoveNode {
            id: NodeId::from(fresh[1].cid.as_str()),
            new_parent: Some(NodeId::from(first.as_str())),
            index: 0,
        },
    ] {
        let expected = oracle.apply_content_operation(&op);
        let actual = session.apply_content_operation(&op);
        assert_eq!(format!("{expected:?}"), format!("{actual:?}"));
        if let Some(index) = &session.session_index {
            assert_eq!(
                index,
                &PlacementIndex::from_placements(session_tree(&session), &fresh_placements(&session))
            );
        }
    }
    assert_eq!(oracle.export_snapshot().unwrap(), session.export_snapshot().unwrap());
}

/// A change made behind the session's back through `&mut self` methods other than the import
/// (here an atom insert and a revert) also retires the index.
#[test]
fn atom_insert_and_revert_retire_the_index() {
    let base = base_page(11, 4, 20);
    let mut oracle = loaded(&base, true);
    let mut session = loaded(&base, false);
    let checkpoint = session.frontier();
    for engine in [&mut oracle, &mut session] {
        engine.apply_content_operation(&create("one", None, 0)).unwrap();
        engine
            .insert_page_atom("pic", None, 0, "image", &image_attrs())
            .unwrap();
    }
    assert!(session.session_index.is_none());
    for engine in [&mut oracle, &mut session] {
        engine.apply_content_operation(&create("two", None, 0)).unwrap();
        engine.revert_to_frontier(&checkpoint).unwrap();
    }
    assert!(session.session_index.is_none());
    for engine in [&mut oracle, &mut session] {
        engine.apply_content_operation(&create("three", None, 0)).unwrap();
    }
    assert_eq!(
        session.session_index.as_ref().unwrap(),
        &PlacementIndex::from_placements(session_tree(&session), &fresh_placements(&session))
    );
    assert_eq!(oracle.export_snapshot().unwrap(), session.export_snapshot().unwrap());
}

/// Full placement-tree walks of one command build: `blocks` blocks created at the top, each with
/// its text, on a loaded page of `page_blocks` blocks.
fn walks_of_a_command_build(model: u32, page_blocks: usize, blocks: usize, recompute: bool) -> usize {
    let base = base_page(3, model, page_blocks);
    let mut engine = loaded(&base, recompute);
    editor_structure::full_walks::reset();
    for n in 0..blocks {
        let id = format!("new{n}");
        engine.apply_content_operation(&create(&id, None, 0)).unwrap();
        engine.apply_content_operation(&text(&id, "typed")).unwrap();
    }
    editor_structure::full_walks::count()
}

/// One command build reads the placement tree once, whatever it inserts and whatever the page
/// size. On the recompute path the same build walks it at least once per inserted block.
#[test]
fn full_placement_walks_of_a_command_build_do_not_grow_with_blocks_or_page() {
    for model in [4_u32, 5] {
        for page_blocks in [200_usize, 2_000] {
            for blocks in [1_usize, 10, 50] {
                let walks = walks_of_a_command_build(model, page_blocks, blocks, false);
                let reference = walks_of_a_command_build(model, page_blocks, blocks, true);
                println!(
                    "model {model} page {page_blocks} blocks {blocks}: session walks {walks}, recompute walks {reference}"
                );
                assert_eq!(walks, 1, "model {model} page {page_blocks} blocks {blocks}");
                assert!(
                    reference >= blocks,
                    "the reference path no longer walks per block ({reference} walks for {blocks} blocks): the counter is not wired"
                );
            }
        }
    }
}

/// Timing, not an assertion. Run with
/// `cargo test --release -p collab-core placement_session_timing -- --ignored --nocapture`.
/// Method: a structured page (model 4) of N blocks is built once; each run loads it, then times
/// a 50-block command build (create at the top, then its text) on the old and on the new path,
/// alternating, and the median of the runs is reported.
#[test]
#[ignore = "timing measurement, release build"]
fn placement_session_timing() {
    use std::time::Instant;
    for page_blocks in [1_000_usize, 2_700, 5_000] {
        let base = base_page(5, 4, page_blocks);
        let mut old = Vec::new();
        let mut new = Vec::new();
        let mut firsts = Vec::new();
        for _ in 0..9 {
            for recompute in [true, false] {
                let mut engine = loaded(&base, recompute);
                let started = Instant::now();
                let mut first = 0.0;
                for n in 0..50 {
                    let id = format!("timed{n}");
                    engine.apply_content_operation(&create(&id, None, 0)).unwrap();
                    if n == 0 {
                        first = started.elapsed().as_secs_f64() * 1_000.0;
                    }
                    engine.apply_content_operation(&text(&id, "typed")).unwrap();
                }
                let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
                if !recompute {
                    firsts.push(first);
                }
                if recompute { &mut old } else { &mut new }.push(elapsed);
            }
        }
        old.sort_by(f64::total_cmp);
        new.sort_by(f64::total_cmp);
        firsts.sort_by(f64::total_cmp);
        println!(
            "page {page_blocks}: first create of the new path, median {:.1} ms",
            firsts[4]
        );
        println!(
            "page {page_blocks}: old median {:.1} ms (min {:.1}, max {:.1}); new median {:.1} ms (min {:.1}, max {:.1})",
            old[4], old[0], old[8], new[4], new[0], new[8]
        );
    }
}
