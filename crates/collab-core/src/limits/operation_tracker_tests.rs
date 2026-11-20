//! `OperationTracker` against the per-operation snapshot path it replaced.
//!
//! The oracle is `apply_operations_checked_per_operation_snapshot`: a complete snapshot of the
//! page before every operation, then `check_operation`, then the operation. For generated pages
//! (tree only, flat editor, structured models 4 and 5, with atoms and with tree nodes beside the
//! editor) and generated batches (every operation kind, failures in the middle, delete then
//! re-create, moves, text edits with multi-byte characters, batches that cross one limit or two
//! at the first, a middle or the last operation), the tracker path must agree with the oracle on:
//! accepted or refused, how many operations were applied before the result, the error (so the
//! limit kind, the observed and the limit numbers), the semantic snapshot and the exported bytes
//! of the document afterwards.
//!
//! Both sides run on documents built by the same deterministic sequence from the same replica
//! seed, so container ids and exported bytes agree.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::cast_possible_truncation,
    clippy::excessive_nesting,
    clippy::similar_names,
    clippy::collapsible_if
)]

use std::collections::BTreeMap;

use super::{
    DocumentLimits, OperationTracker, apply_operations_checked, apply_operations_checked_per_operation_snapshot,
    run_checked,
};
use crate::engine::{CollabEngine, LoroCollabEngine};
use crate::error::CollabError;
use crate::operation::{NodeId, NodeKind, Operation};
use crate::semantic::SemanticSnapshot;

const REPLICA: u64 = 0x0005_eed5;
const ASSET: &str = "4f8d0e0a-2f1b-4c1a-9d35-0d6f4f3d9a10";

pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        let mut rng = Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        for _ in 0..4 {
            rng.next();
        }
        rng
    }

    pub(super) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform in `0..n` (`n > 0`).
    pub(super) fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub(super) fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    pub(super) fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            items.get(self.below(items.len()))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Page {
    Tree,
    Flat,
    Structured4,
    Structured5,
}

pub(super) const PAGES: [Page; 4] = [Page::Tree, Page::Flat, Page::Structured4, Page::Structured5];

#[derive(Default, Clone)]
pub(super) struct Pool {
    pub(super) editor: Vec<String>,
    pub(super) tree: Vec<String>,
}

const TEXTS: [&str; 8] = [
    "a",
    "hello world",
    "caf\u{e9}",
    "\u{1F600}",
    "x\u{301}y",
    "ab",
    "0123456789",
    "\u{4e2d}\u{6587}",
];

fn create(id: &str, parent: Option<&str>, index: u32, kind: NodeKind) -> Operation {
    Operation::CreateNode {
        id: NodeId::from(id),
        parent: parent.map(NodeId::from),
        index,
        kind,
    }
}

fn insert(id: &str, index: u32, text: &str) -> Operation {
    Operation::InsertText {
        id: NodeId::from(id),
        index,
        text: text.to_owned(),
    }
}

/// A page of `size` nodes, deterministic in `(page, seed, size)`.
pub(super) fn build(page: Page, seed: u64, size: usize) -> (LoroCollabEngine, Pool) {
    let mut rng = Rng::new(seed ^ 0xB11D);
    let mut engine = LoroCollabEngine::new_empty(REPLICA);
    let mut pool = Pool::default();
    if page != Page::Tree {
        engine.initialize_page_editor().unwrap();
    }
    if matches!(page, Page::Structured4 | Page::Structured5) {
        engine
            .initialize_page_structure_model(if page == Page::Structured4 { 4 } else { 5 })
            .unwrap();
    }
    if page == Page::Structured5 {
        engine
            .insert_page_atom(
                "seed-image",
                None,
                0,
                "image",
                &serde_json::json!({"asset_id": ASSET, "alt": "an image", "caption": "", "indent": 0}),
            )
            .unwrap();
        pool.editor.push("seed-image".to_owned());
    }
    let structured = matches!(page, Page::Structured4 | Page::Structured5);
    for index in 0..size {
        let id = format!("b{index}");
        let editor = page != Page::Tree && rng.below(8) != 0;
        let operation = if editor {
            let parent = if structured && rng.chance(40) {
                rng.pick(&pool.editor).cloned()
            } else {
                None
            };
            create(&id, parent.as_deref(), 0, NodeKind::Block)
        } else {
            let parent = if rng.chance(60) {
                rng.pick(&pool.tree).cloned()
            } else {
                None
            };
            let kind = match rng.below(6) {
                0 => NodeKind::NavigatorNode,
                1 => NodeKind::CollectionField,
                _ => NodeKind::Block,
            };
            create(&id, parent.as_deref(), 0, kind)
        };
        let kind_is_block = matches!(
            operation,
            Operation::CreateNode {
                kind: NodeKind::Block,
                ..
            }
        );
        if engine.apply_content_operation(&operation).is_ok() {
            if editor {
                pool.editor.push(id.clone());
            } else {
                pool.tree.push(id.clone());
            }
            if kind_is_block && rng.chance(75) {
                let text = *rng.pick(&TEXTS).unwrap();
                let _ = engine.apply_content_operation(&insert(&id, 0, text));
            }
        }
        if rng.chance(6) {
            if let Some(victim) = rng.pick(&pool.editor).or_else(|| pool.tree.first()).cloned() {
                let _ = engine.apply_content_operation(&Operation::DeleteNode {
                    id: NodeId::from(victim.as_str()),
                });
            }
        }
    }
    (engine, pool)
}

fn kind_of(rng: &mut Rng) -> NodeKind {
    match rng.below(10) {
        0 => NodeKind::NavigatorNode,
        1 => NodeKind::CollectionField,
        2 => NodeKind::CollectionView,
        3 => NodeKind::RecordProperty,
        _ => NodeKind::Block,
    }
}

/// A random batch against `pool`. Ids come from the pool (by logical id and by container id),
/// from earlier operations of the batch, or are unknown; indices and lengths are sometimes
/// invalid, so operations fail validation at random positions. With `valid` the batch is mostly
/// well formed (text edits at character boundaries of text the batch knows, parents that exist),
/// so that it runs deep enough to cross limits; without it most batches fail on their first
/// operations.
fn random_batch(
    rng: &mut Rng,
    page: Page,
    pool: &Pool,
    cids: &[String],
    texts: &mut Vec<(String, String)>,
    operations: usize,
    valid: bool,
) -> Vec<Operation> {
    let mut fresh_editor: Vec<String> = Vec::new();
    let mut fresh_tree: Vec<String> = Vec::new();
    let mut fresh: Vec<String> = Vec::new();
    let mut batch = Vec::new();
    for position in 0..operations {
        let mut id = pick_id(rng, pool, cids, &fresh);
        if valid {
            let known: Vec<String> = if page == Page::Tree {
                pool.tree.iter().chain(fresh_tree.iter()).cloned().collect()
            } else {
                pool.editor
                    .iter()
                    .chain(fresh_editor.iter())
                    .chain(cids.iter())
                    .cloned()
                    .collect()
            };
            if let Some(known_id) = rng.pick(&known) {
                id = known_id.clone();
            }
        }
        let mut roll = rng.below(100);
        if valid && texts.is_empty() && (30..65).contains(&roll) {
            roll = rng.below(30);
        }
        let operation = if valid && (30..65).contains(&roll) && !texts.is_empty() {
            let slot = rng.below(texts.len());
            let (target, text) = &mut texts[slot];
            let boundaries: Vec<usize> = (0..=text.len()).filter(|at| text.is_char_boundary(*at)).collect();
            let start = boundaries[rng.below(boundaries.len())];
            if roll < 52 || text.is_empty() {
                let added = TEXTS[rng.below(TEXTS.len())];
                text.insert_str(start, added);
                insert(target, start as u32, added)
            } else {
                let ends: Vec<usize> = boundaries.iter().copied().filter(|end| *end >= start).collect();
                let end = ends[rng.below(ends.len())];
                text.replace_range(start..end, "");
                Operation::DeleteText {
                    id: NodeId::from(target.as_str()),
                    index: start as u32,
                    len: (end - start) as u32,
                }
            }
        } else {
            match roll {
                0..=29 => {
                    let new_id = if !valid && rng.chance(8) {
                        id.clone()
                    } else {
                        let new_id = format!("n{position}");
                        fresh.push(new_id.clone());
                        new_id
                    };
                    let kind = if valid && rng.chance(70) {
                        NodeKind::Block
                    } else {
                        kind_of(rng)
                    };
                    let editor_block = page != Page::Tree && kind == NodeKind::Block;
                    let structured = matches!(page, Page::Structured4 | Page::Structured5);
                    let parent = if valid {
                        if editor_block {
                            if structured && rng.chance(40) {
                                let all: Vec<String> = pool.editor.iter().chain(fresh_editor.iter()).cloned().collect();
                                rng.pick(&all).cloned()
                            } else {
                                None
                            }
                        } else if rng.chance(55) {
                            let all: Vec<String> = pool.tree.iter().chain(fresh_tree.iter()).cloned().collect();
                            rng.pick(&all).cloned()
                        } else {
                            None
                        }
                    } else if rng.chance(55) {
                        None
                    } else {
                        Some(pick_id(rng, pool, cids, &fresh))
                    };
                    if editor_block {
                        fresh_editor.push(new_id.clone());
                    } else {
                        fresh_tree.push(new_id.clone());
                    }
                    if kind == NodeKind::Block {
                        texts.push((new_id.clone(), String::new()));
                    }
                    create(
                        &new_id,
                        parent.as_deref(),
                        if valid { 0 } else { rng.below(3) as u32 },
                        kind,
                    )
                }
                30..=54 => insert(&id, rng.below(12) as u32, rng.pick(&TEXTS).copied().unwrap_or("t")),
                55..=64 => Operation::DeleteText {
                    id: NodeId::from(id.as_str()),
                    index: rng.below(8) as u32,
                    len: rng.below(8) as u32,
                },
                65..=74 => {
                    let (key, value) = match rng.below(9) {
                        0 => ("indent", ["0", "3", "9", "x"][rng.below(4)].to_owned()),
                        1 => ("level", ["1", "2", "7"][rng.below(3)].to_owned()),
                        2 => (
                            "block_type",
                            ["paragraph", "heading", "divider", "image", "codeBlock", "bulletItem"][rng.below(6)]
                                .to_owned(),
                        ),
                        3 => ("alt", TEXTS[rng.below(TEXTS.len())].repeat(1 + rng.below(3))),
                        4 => ("checked", "true".to_owned()),
                        5 => ("order", "3".to_owned()),
                        6 => ("content_source", "editor_doc".to_owned()),
                        7 => ("caption", TEXTS[rng.below(TEXTS.len())].to_owned()),
                        _ => ("foo", "bar".to_owned()),
                    };
                    Operation::SetProperty {
                        id: NodeId::from(id.as_str()),
                        key: key.to_owned(),
                        value,
                    }
                }
                75..=84 if !(valid && rng.chance(60)) => Operation::DeleteNode {
                    id: NodeId::from(id.as_str()),
                },
                _ => {
                    if valid && (page == Page::Flat || rng.chance(50)) {
                        insert(&id, 0, "m")
                    } else {
                        let parent = if rng.chance(40) {
                            None
                        } else {
                            Some(NodeId::from(pick_id(rng, pool, cids, &fresh).as_str()))
                        };
                        Operation::MoveNode {
                            id: NodeId::from(id.as_str()),
                            new_parent: parent,
                            index: if valid { 0 } else { rng.below(3) as u32 },
                        }
                    }
                }
            }
        };
        batch.push(operation);
    }
    batch
}

/// The text of every block of the page the batch can address, by logical id.
fn known_texts(engine: &LoroCollabEngine, pool: &Pool, page: Page) -> Vec<(String, String)> {
    let snapshot = engine.semantic_snapshot().unwrap_or_default();
    let mut texts = Vec::new();
    if page == Page::Tree {
        for id in &pool.tree {
            if let Some(node) = snapshot.nodes.get(id.as_str())
                && node.kind == NodeKind::Block
                && !node.deleted
            {
                texts.push((id.clone(), node.text.clone()));
            }
        }
    } else {
        for id in &pool.editor {
            let cid = engine.resolve_content_node_id(&NodeId::from(id.as_str())).unwrap();
            if let Some(node) = snapshot.nodes.get(&cid)
                && is_prose(node)
            {
                texts.push((id.clone(), node.text.clone()));
            }
        }
    }
    texts
}

fn is_prose(node: &crate::semantic::SemanticNode) -> bool {
    node.properties
        .get("block_type")
        .is_some_and(|kind| !matches!(kind.as_str(), "image" | "attachment" | "childPage" | "divider"))
}

fn pick_id(rng: &mut Rng, pool: &Pool, cids: &[String], fresh: &[String]) -> String {
    let roll = rng.below(100);
    let choice = match roll {
        0..=34 => rng.pick(fresh),
        35..=64 => rng.pick(&pool.editor),
        65..=74 => rng.pick(cids),
        75..=89 => rng.pick(&pool.tree),
        _ => None,
    };
    choice.cloned().unwrap_or_else(|| format!("unknown{}", rng.below(3)))
}

fn cids_of(engine: &LoroCollabEngine, pool: &Pool) -> Vec<String> {
    pool.editor
        .iter()
        .map(|id| {
            engine
                .resolve_content_node_id(&NodeId::from(id.as_str()))
                .unwrap()
                .to_string()
        })
        .filter(|cid| cid.starts_with("cid:"))
        .collect()
}

/// Limits near what `snapshot` already holds, so that a short batch can cross them.
fn near_limits(rng: &mut Rng, snapshot: &SemanticSnapshot) -> DocumentLimits {
    let blocks = snapshot
        .nodes
        .values()
        .filter(|n| !n.deleted && n.kind == NodeKind::Block)
        .count();
    let navs = snapshot
        .nodes
        .values()
        .filter(|n| !n.deleted && n.kind == NodeKind::NavigatorNode)
        .count();
    let chars: usize = snapshot
        .nodes
        .values()
        .filter(|n| !n.deleted)
        .map(|n| n.text.chars().count())
        .sum();
    let longest = snapshot
        .nodes
        .values()
        .filter(|n| !n.deleted)
        .map(|n| n.text.chars().count())
        .max()
        .unwrap_or(0);
    let mut limits = DocumentLimits::default();
    if rng.chance(70) {
        limits.tree_depth_max = 1 + rng.below(6);
    }
    if rng.chance(70) {
        limits.document_block_count_max = blocks + rng.below(5);
    }
    if rng.chance(50) {
        limits.container_count_max = navs + rng.below(4);
    }
    if rng.chance(60) {
        limits.text_block_chars_max = longest + rng.below(8);
    }
    if rng.chance(60) {
        limits.document_text_chars_max = chars + rng.below(12);
    }
    limits
}

#[derive(Default)]
pub(super) struct Stats {
    pub(super) cases: usize,
    pub(super) operations: usize,
    pub(super) accepted: usize,
    pub(super) refused_by_limit: BTreeMap<&'static str, usize>,
    pub(super) refused_by_engine: usize,
    pub(super) refused_at_first: usize,
    pub(super) refused_in_the_middle: usize,
    pub(super) refused_at_last: usize,
    pub(super) with_rebuild: usize,
    pub(super) single_view: usize,
    pub(super) by_page: BTreeMap<String, usize>,
    pub(super) engine_reasons: BTreeMap<String, usize>,
}

impl Stats {
    fn add(&mut self, page: Page, length: usize, applied: usize, result: &Result<(), CollabError>, views: usize) {
        self.cases += 1;
        self.operations += length;
        *self.by_page.entry(format!("{page:?}")).or_default() += 1;
        match result {
            Ok(()) => self.accepted += 1,
            Err(CollabError::LimitExceeded { limit_kind, .. }) => {
                *self.refused_by_limit.entry(limit_kind).or_default() += 1;
                match applied {
                    0 => self.refused_at_first += 1,
                    n if n + 1 == length => self.refused_at_last += 1,
                    _ => self.refused_in_the_middle += 1,
                }
            }
            Err(error) => {
                self.refused_by_engine += 1;
                *self
                    .engine_reasons
                    .entry(format!("{error:?}").chars().take(70).collect())
                    .or_default() += 1;
            }
        }
        if views > 1 {
            self.with_rebuild += 1;
        } else {
            self.single_view += 1;
        }
    }
}

/// Runs `batch` on two identical builds, one per path, and compares everything observable.
/// Returns the tracker's number of views and the shared result.
pub(super) fn compare(
    page: Page,
    seed: u64,
    size: usize,
    limits: &DocumentLimits,
    batch: &[Operation],
    label: &str,
) -> (usize, usize, Result<(), CollabError>) {
    let (mut oracle, _) = build(page, seed, size);
    let (mut tracked, _) = build(page, seed, size);
    assert_eq!(
        oracle.export_snapshot().unwrap(),
        tracked.export_snapshot().unwrap(),
        "{label}: the two builds differ before the batch"
    );
    let (expected, expected_applied) = apply_operations_checked_per_operation_snapshot(&mut oracle, batch, limits);
    let mut tracker = OperationTracker::new(None);
    let mut applied = 0;
    let actual = run_checked(&mut tracked, batch, limits, &mut tracker, &mut applied);
    assert_eq!(
        actual, expected,
        "{label}: result differs\nlimits {limits:?}\nbatch {batch:#?}"
    );
    assert_eq!(applied, expected_applied, "{label}: failing operation index differs");
    assert_eq!(
        tracked.semantic_snapshot(),
        oracle.semantic_snapshot(),
        "{label}: resulting semantic snapshot differs"
    );
    assert_eq!(
        tracked.export_snapshot().unwrap(),
        oracle.export_snapshot().unwrap(),
        "{label}: resulting document bytes differ"
    );
    // The public entry point, with the caller's own seed snapshot, is the same function.
    let (mut seeded, _) = build(page, seed, size);
    // Exported before the batch like the two builds above: an export fixes how the bytes encode.
    let _ = seeded.export_snapshot().unwrap();
    let seed_snapshot = seeded.semantic_snapshot().ok();
    let via_entry = apply_operations_checked(&mut seeded, batch, limits, seed_snapshot);
    assert_eq!(via_entry, expected, "{label}: the seeded entry point differs");
    let (a, b) = (seeded.export_snapshot().unwrap(), oracle.export_snapshot().unwrap());
    assert_eq!(a, b, "{label}: the seeded entry point leaves other bytes");
    (tracker.views_built(), applied, actual)
}

/// Generated pages and batches: `cases` cases from seeds `first..first + cases`.
pub(super) fn run_random(first: u64, cases: u64) -> Stats {
    let mut stats = Stats::default();
    for seed in first..first + cases {
        let mut rng = Rng::new(seed);
        let page = PAGES[(seed % 4) as usize];
        let size = match rng.below(10) {
            0 => 0,
            1..=6 => 1 + rng.below(12),
            _ => 12 + rng.below(30),
        };
        let (engine, pool) = build(page, seed, size);
        let cids = cids_of(&engine, &pool);
        let snapshot = engine.semantic_snapshot().unwrap_or_default();
        let limits = if rng.chance(15) {
            DocumentLimits::default()
        } else {
            near_limits(&mut rng, &snapshot)
        };
        let long = rng.chance(15);
        let length = 1 + rng.below(if long { 25 } else { 8 });
        let valid = !rng.chance(25);
        let mut texts = known_texts(&engine, &pool, page);
        let batch = random_batch(&mut rng, page, &pool, &cids, &mut texts, length, valid);
        let (views, applied, result) = compare(page, seed, size, &limits, &batch, &format!("seed {seed} {page:?}"));
        stats.add(page, batch.len(), applied, &result, views);
    }
    stats
}

#[test]
fn tracker_decisions_equal_per_operation_snapshots_on_generated_batches() {
    let cases: u64 = std::env::var("TRACKER_DIFFERENTIAL_CASES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_200);
    let first: u64 = std::env::var("TRACKER_DIFFERENTIAL_FIRST_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    let stats = run_random(first, cases);
    println!(
        "random: seeds {first}..{} cases {} operations {} accepted {} limit refusals {:?} engine refusals {} \
         refused at first/middle/last {}/{}/{} rebuilt {} single-view {} pages {:?}",
        first + cases,
        stats.cases,
        stats.operations,
        stats.accepted,
        stats.refused_by_limit,
        stats.refused_by_engine,
        stats.refused_at_first,
        stats.refused_in_the_middle,
        stats.refused_at_last,
        stats.with_rebuild,
        stats.single_view,
        stats.by_page
    );
    println!("engine reasons {:#?}", stats.engine_reasons);
    if cases >= 1_000 {
        for kind in [
            "tree_depth",
            "document_block_count",
            "text_block_chars",
            "document_text_chars",
        ] {
            assert!(
                stats.refused_by_limit.get(kind).copied().unwrap_or(0) >= 5,
                "the generator crossed {kind} too rarely: {:?}",
                stats.refused_by_limit
            );
        }
        assert!(stats.refused_at_first > 0 && stats.refused_in_the_middle > 0 && stats.refused_at_last > 0);
        assert!(stats.accepted > 100 && stats.refused_by_engine > 100);
    }
}

#[derive(Debug, Clone, Copy)]
enum Scenario {
    BlockCount,
    ContainerCount,
    BlockChars,
    DocumentChars,
    Depth,
    DepthAndBlockCount,
    BlockCharsAndDocumentChars,
    DeleteThenRecreate,
    MoveCrossesDepth,
}

const SCENARIOS: [Scenario; 9] = [
    Scenario::BlockCount,
    Scenario::ContainerCount,
    Scenario::BlockChars,
    Scenario::DocumentChars,
    Scenario::Depth,
    Scenario::DepthAndBlockCount,
    Scenario::BlockCharsAndDocumentChars,
    Scenario::DeleteThenRecreate,
    Scenario::MoveCrossesDepth,
];

struct Totals {
    blocks: usize,
    navs: usize,
    chars: usize,
}

fn totals(engine: &LoroCollabEngine) -> Totals {
    let snapshot = engine.semantic_snapshot().unwrap_or_default();
    let live = snapshot.nodes.values().filter(|n| !n.deleted);
    Totals {
        blocks: live.clone().filter(|n| n.kind == NodeKind::Block).count(),
        navs: live.clone().filter(|n| n.kind == NodeKind::NavigatorNode).count(),
        chars: live.map(|n| n.text.chars().count()).sum(),
    }
}

/// A batch of `n` operations built to cross the limits of `scenario` at operation `crossing`
/// (0-based, within the batch), and the limits it crosses. Filler operations before the crossing
/// stay inside every limit.
fn directed_case(
    page: Page,
    seed: u64,
    size: usize,
    scenario: Scenario,
    n: usize,
    crossing: usize,
) -> (DocumentLimits, Vec<Operation>) {
    let (engine, pool) = build(page, seed, size);
    let have = totals(&engine);
    let mut limits = DocumentLimits::default();
    let mut batch = Vec::new();
    // Where the new text goes: an existing prose block of the page, else a block created first.
    let texts = known_texts(&engine, &pool, page);
    let longest = texts.iter().map(|(_, text)| text.chars().count()).max().unwrap_or(0);
    let existing = texts
        .iter()
        .find(|(_, text)| text.chars().count() == longest)
        .map(|(id, _)| id.clone());
    let (target, target_len) = existing.map_or_else(
        || {
            batch.push(create("target", None, 0, NodeKind::Block));
            ("target".to_owned(), 0)
        },
        |id| (id, longest),
    );
    let lead = batch.len();
    match scenario {
        Scenario::BlockCount | Scenario::DepthAndBlockCount => {
            limits.document_block_count_max = have.blocks + lead + crossing;
            let tree_page = page == Page::Tree;
            if matches!(scenario, Scenario::DepthAndBlockCount) {
                limits.tree_depth_max = crossing.saturating_sub(1);
            }
            for index in 0..n {
                let id = format!("c{index}");
                // A chain: each node under the previous one, so the depth grows with the index.
                let parent = (tree_page && index > 0).then(|| format!("c{}", index - 1));
                batch.push(create(&id, parent.as_deref(), 0, NodeKind::Block));
                batch.push(insert(&id, 0, "ab"));
            }
        }
        Scenario::ContainerCount => {
            limits.container_count_max = have.navs + crossing;
            for index in 0..n {
                batch.push(create(&format!("c{index}"), None, 0, NodeKind::NavigatorNode));
                batch.push(insert(&target, 0, "z"));
            }
        }
        Scenario::BlockChars | Scenario::BlockCharsAndDocumentChars => {
            limits.text_block_chars_max = target_len + 2 * crossing + 1;
            if matches!(scenario, Scenario::BlockCharsAndDocumentChars) {
                limits.document_text_chars_max = have.chars + 2 * crossing + 1;
            }
            for _ in 0..n {
                batch.push(insert(&target, 0, "xy"));
            }
        }
        Scenario::DocumentChars => {
            limits.document_text_chars_max = have.chars + 2 * crossing + 1;
            for index in 0..n {
                let id = format!("c{index}");
                batch.push(create(&id, None, 0, NodeKind::Block));
                batch.push(insert(&id, 0, "xy"));
                batch.push(insert(&target, 0, ""));
            }
        }
        Scenario::Depth => {
            limits.tree_depth_max = crossing;
            for index in 0..n {
                let parent = (index > 0).then(|| format!("c{}", index - 1));
                // Tree nodes of a kind that is not an editor block, so every page builds a chain.
                batch.push(create(
                    &format!("c{index}"),
                    parent.as_deref(),
                    0,
                    NodeKind::CollectionField,
                ));
            }
        }
        Scenario::DeleteThenRecreate => {
            limits.document_block_count_max = have.blocks + lead;
            let victim = pool
                .editor
                .first()
                .or_else(|| pool.tree.first())
                .cloned()
                .unwrap_or_else(|| target.clone());
            batch.push(Operation::DeleteNode {
                id: NodeId::from(victim.as_str()),
            });
            for index in 0..n.max(2) {
                batch.push(create(&format!("r{index}"), None, 0, NodeKind::Block));
            }
            batch.push(create(&victim, None, 0, NodeKind::Block));
        }
        Scenario::MoveCrossesDepth => {
            limits.tree_depth_max = 2;
            for index in 0..n {
                let parent = (index > 0).then(|| format!("c{}", index - 1));
                batch.push(create(
                    &format!("c{index}"),
                    parent.as_deref(),
                    0,
                    NodeKind::CollectionField,
                ));
                if index == crossing.min(n - 1) {
                    batch.push(Operation::MoveNode {
                        id: NodeId::from("c0"),
                        new_parent: Some(NodeId::from(format!("c{}", n - 1).as_str())),
                        index: 0,
                    });
                }
            }
        }
    }
    (limits, batch)
}

#[test]
fn tracker_decisions_equal_per_operation_snapshots_on_directed_limit_crossings() {
    let mut stats = Stats::default();
    for page in PAGES {
        for size in [0_usize, 1, 6, 30] {
            for (index, scenario) in SCENARIOS.iter().copied().enumerate() {
                for n in [3_usize, 6] {
                    for crossing in [0, n / 2, n - 1] {
                        for seed in 0..2_u64 {
                            let seed = seed + 100 * index as u64;
                            let (limits, batch) = directed_case(page, seed, size, scenario, n, crossing);
                            let label =
                                format!("{page:?} {scenario:?} size {size} n {n} crossing {crossing} seed {seed}");
                            let (views, applied, result) = compare(page, seed, size, &limits, &batch, &label);
                            stats.add(page, batch.len(), applied, &result, views);
                        }
                    }
                }
            }
        }
    }
    println!(
        "directed: cases {} operations {} accepted {} limit refusals {:?} engine refusals {} refused at \
         first/middle/last {}/{}/{} rebuilt {} single-view {}",
        stats.cases,
        stats.operations,
        stats.accepted,
        stats.refused_by_limit,
        stats.refused_by_engine,
        stats.refused_at_first,
        stats.refused_in_the_middle,
        stats.refused_at_last,
        stats.with_rebuild,
        stats.single_view
    );
    for kind in [
        "tree_depth",
        "container_count",
        "document_block_count",
        "text_block_chars",
        "document_text_chars",
    ] {
        assert!(
            stats.refused_by_limit.get(kind).copied().unwrap_or(0) >= 20,
            "the directed cases crossed {kind} too rarely: {:?}",
            stats.refused_by_limit
        );
    }
    assert!(stats.refused_at_first > 0 && stats.refused_in_the_middle > 0 && stats.refused_at_last > 0);
}
