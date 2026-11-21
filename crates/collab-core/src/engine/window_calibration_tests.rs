//! Calibration of the work budget (`crate::work`): the cost of the full window against the items
//! the full projection reads, for the realistic pages and the growth shapes. Ignored by default;
//! run in release: `cargo test --release -p collab-core --lib window_calibration -- --ignored
//! --nocapture --test-threads=1`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::similar_names
)]

use std::time::{Duration, Instant};

use loro::{Container, ExportMode, LoroDoc, LoroMap, TreeID, TreeParentId, ValueOrContainer};

use super::floor_equivalence_tests::children;
use super::hand_encoded::{raw_edit, set_node};
use super::window_fixtures::{BaseSpec, History, Model, build_base};
use super::{CollabEngine, LoroCollabEngine};
use crate::editor_doc_bridge::{EditorInitialBlock, initialize_editor_doc};
use crate::limits::{DocumentLimits, check_snapshot};

/// The full window as the worker runs it without facts, in milliseconds, and the items of the base.
fn measure(label: &str, base: &[u8], update: &[u8]) {
    let mut best = Duration::MAX;
    let mut items = 0;
    let mut live = (0, 0, 0);
    for _ in 0..5 {
        let mut engine = LoroCollabEngine::load(base).unwrap();
        engine.prewarm_for_writes();
        items = engine.projection_items(u64::MAX).unwrap().unwrap();
        let started = Instant::now();
        engine.import_client_update(update).unwrap();
        let semantic = engine.semantic_snapshot().unwrap();
        check_snapshot(&semantic, &DocumentLimits::default()).unwrap();
        best = best.min(started.elapsed());
        let live_nodes = semantic.nodes.values().filter(|n| !n.deleted);
        live = (
            live_nodes
                .clone()
                .filter(|n| n.kind == crate::operation::NodeKind::Block)
                .count(),
            live_nodes
                .clone()
                .filter(|n| n.kind == crate::operation::NodeKind::NavigatorNode)
                .count(),
            live_nodes.map(|n| n.text.chars().count()).sum::<usize>(),
        );
    }
    let marks = LoroCollabEngine::load(base)
        .unwrap()
        .unit_mark_items()
        .unwrap()
        .values()
        .sum();
    let budget = crate::work::item_budget_with_marks(live.0 as u64, live.1 as u64, live.2 as u64, marks);
    println!(
        "{label:<46} items {items:>8}  budget {budget:>8}  live (blocks, navs, chars) {live:?}  full window {:>8.2} ms  {:>6.3} us/item  reference x3.6 = {:>7.1} ms",
        best.as_secs_f64() * 1e3,
        best.as_secs_f64() * 1e6 / items.max(1) as f64,
        best.as_secs_f64() * 1e3 * 3.6
    );
}

fn trivial_update(base: &[u8]) -> Vec<u8> {
    raw_edit(base, 424_242, |raw| {
        let tree = raw.get_tree("tree");
        tree.enable_fractional_index(0);
        let n = tree.create(TreeParentId::Root).unwrap();
        set_node(raw, n, "calibration_probe", "navigator_node");
    })
}

fn flat_realistic(blocks: usize) -> Vec<u8> {
    build_base(BaseSpec {
        model: Model::Flat,
        blocks,
        seed: 5,
        history: History::None,
    })
    .snapshot
    .as_ref()
    .clone()
}

fn legacy_realistic(blocks: usize) -> Vec<u8> {
    build_base(BaseSpec {
        model: Model::LegacyTree,
        blocks,
        seed: 6,
        history: History::None,
    })
    .snapshot
    .as_ref()
    .clone()
}

fn tree_of(kind: &str, count: usize, under_deleted: bool, props: usize) -> Vec<u8> {
    let engine = LoroCollabEngine::new_empty(77);
    let tree = engine.doc.get_tree("tree");
    tree.enable_fractional_index(0);
    let trash = tree.create(TreeParentId::Root).unwrap();
    set_node(&engine.doc, trash, "trash", "navigator_node");
    for i in 0..count {
        let parent = if under_deleted {
            TreeParentId::Node(trash)
        } else {
            TreeParentId::Root
        };
        let n = tree.create(parent).unwrap();
        set_node(&engine.doc, n, &format!("g{i}"), kind);
        let meta = tree.get_meta(n).unwrap();
        for p in 0..props {
            meta.insert(&format!("prop:k{p}"), "v").unwrap();
        }
    }
    if under_deleted {
        tree.delete(trash).unwrap();
    }
    engine.doc.commit();
    engine.export_snapshot().unwrap()
}

fn flat_with_attribute_keys(blocks: usize, keys: usize) -> Vec<u8> {
    let engine = LoroCollabEngine::new_empty(61);
    let list: Vec<EditorInitialBlock> = (0..blocks)
        .map(|index| EditorInitialBlock {
            id: format!("b{index}"),
            kind: "paragraph".into(),
            text: "x".into(),
            indent: 0,
            level: None,
        })
        .collect();
    initialize_editor_doc(&engine.doc, &list).unwrap();
    engine.doc.commit();
    let Some(ValueOrContainer::Container(Container::Map(block))) = children(&engine.doc).get(0) else {
        panic!()
    };
    let attrs = match block.get("attributes") {
        Some(ValueOrContainer::Container(Container::Map(a))) => a,
        _ => block.insert_container("attributes", LoroMap::new()).unwrap(),
    };
    for i in 0..keys {
        attrs.insert(&format!("k{i}"), i64::try_from(i).unwrap()).unwrap();
    }
    engine.doc.commit();
    engine.export_snapshot().unwrap()
}

fn flat_with_unsupported(blocks: usize, extra: usize) -> Vec<u8> {
    let engine = LoroCollabEngine::new_empty(11);
    let list: Vec<EditorInitialBlock> = (0..blocks)
        .map(|index| EditorInitialBlock {
            id: format!("b{index}"),
            kind: "paragraph".into(),
            text: "x".repeat(10),
            indent: 0,
            level: None,
        })
        .collect();
    initialize_editor_doc(&engine.doc, &list).unwrap();
    let list = children(&engine.doc);
    for _ in 0..extra {
        let map = list.insert_container(list.len(), LoroMap::new()).unwrap();
        map.insert("nodeName", "z").unwrap();
    }
    engine.doc.commit();
    engine.export_snapshot().unwrap()
}

#[test]
#[ignore = "calibration, run in release"]
fn window_calibration() {
    for blocks in [1_000, 5_000, 9_950] {
        let base = flat_realistic(blocks);
        measure(&format!("flat realistic {blocks}"), &base, &trivial_update(&base));
    }
    for blocks in [1_000, 5_000, 10_000] {
        let base = legacy_realistic(blocks);
        measure(&format!("legacy realistic {blocks}"), &base, &trivial_update(&base));
    }
    for count in [1_000, 2_000, 4_000, 8_000, 16_000, 32_000] {
        let base = tree_of("collection_field", count, false, 0);
        measure(
            &format!("collection_field nodes {count}"),
            &base,
            &trivial_update(&base),
        );
    }
    for count in [2_000, 8_000, 16_000] {
        let base = tree_of("navigator_node", count, true, 0);
        measure(
            &format!("navigators under a deleted parent {count}"),
            &base,
            &trivial_update(&base),
        );
    }
    for props in [20, 100] {
        let base = tree_of("collection_field", 2_000, false, props);
        measure(&format!("2000 nodes x {props} props"), &base, &trivial_update(&base));
    }
    for keys in [10_000, 50_000, 200_000] {
        let base = flat_with_attribute_keys(400, keys);
        measure(&format!("attributes keys {keys}"), &base, &trivial_update(&base));
    }
    for extra in [2_000, 8_000, 32_000] {
        let base = flat_with_unsupported(10, extra);
        measure(&format!("unsupported maps {extra}"), &base, &trivial_update(&base));
    }
    let _ = (LoroDoc::new(), TreeID::new(0, 0));
}

/// A flat page of 400 blocks whose `attributes` hold `count` values made by `value`, put in by
/// updates of at most 64 KB.
fn flat_with_values(count: usize, value: &dyn Fn(usize) -> loro::LoroValue) -> Vec<u8> {
    let mut base = flat_page_of_blocks(400);
    for index in 0..count {
        let update = raw_edit(&base, 90_000 + index as u64, |raw| {
            let list = children(raw);
            let Some(ValueOrContainer::Container(Container::Map(block))) = list.get(index % 400) else {
                panic!()
            };
            let attributes = match block.get("attributes") {
                Some(ValueOrContainer::Container(Container::Map(a))) => a,
                _ => block.insert_container("attributes", LoroMap::new()).unwrap(),
            };
            attributes.insert(&format!("p{index}"), value(index)).unwrap();
        });
        assert!(update.len() <= 65_536, "{} bytes", update.len());
        base = super::hand_encoded::accepted(&base, &update).as_ref().clone();
    }
    base
}

fn flat_page_of_blocks(blocks: usize) -> Vec<u8> {
    let engine = LoroCollabEngine::new_empty(61);
    let list: Vec<EditorInitialBlock> = (0..blocks)
        .map(|index| EditorInitialBlock {
            id: format!("b{index}"),
            kind: "paragraph".into(),
            text: "x".into(),
            indent: 0,
            level: None,
        })
        .collect();
    initialize_editor_doc(&engine.doc, &list).unwrap();
    engine.doc.commit();
    engine.export_snapshot().unwrap()
}

/// The cost of the full window per item with payload in plain values, for the shapes that cost the
/// most per byte of payload: it is what `work::BYTES_PER_ITEM` is chosen from.
#[test]
#[ignore = "calibration, run in release"]
fn window_payload_calibration() {
    use loro::LoroValue;
    let page = flat_page_of_blocks(400);
    measure("flat 400, no payload", &page, &trivial_update(&page));
    let ints = |n: usize| -> LoroValue { (0..i64::try_from(n).unwrap()).map(|i| i % 7).collect::<Vec<_>>().into() };
    for writes in [2, 4, 6, 8, 16] {
        let base = flat_with_values(writes, &|_| ints(20_000));
        measure(&format!("{writes} lists of 20,000 ints"), &base, &trivial_update(&base));
    }
    for writes in [4, 16, 32] {
        let base = flat_with_values(writes, &|_| "s".repeat(60_000).into());
        measure(&format!("{writes} strings of 60,000"), &base, &trivial_update(&base));
    }
    // Many tiny maps: the most elements per byte of payload.
    let tiny_maps = |n: usize| -> LoroValue {
        (0..n)
            .map(|i| {
                let mut entry = std::collections::HashMap::new();
                entry.insert("a".to_owned(), LoroValue::from(i64::try_from(i).unwrap() % 5));
                LoroValue::from(entry)
            })
            .collect::<Vec<_>>()
            .into()
    };
    for writes in [2, 4, 8] {
        let base = flat_with_values(writes, &|_| tiny_maps(4_000));
        measure(
            &format!("{writes} lists of 4,000 one-key maps"),
            &base,
            &trivial_update(&base),
        );
    }
    // Lists of lists of one integer each.
    let nested = |n: usize| -> LoroValue {
        (0..n)
            .map(|i| LoroValue::from(vec![LoroValue::from(i64::try_from(i).unwrap() % 5)]))
            .collect::<Vec<_>>()
            .into()
    };
    for writes in [2, 4, 8] {
        let base = flat_with_values(writes, &|_| nested(8_000));
        measure(
            &format!("{writes} lists of 8,000 one-int lists"),
            &base,
            &trivial_update(&base),
        );
    }
    // Maps with many short keys.
    let wide = |n: usize| -> LoroValue {
        let entries: std::collections::HashMap<String, LoroValue> = (0..n)
            .map(|i| (format!("k{i}"), LoroValue::from(i64::try_from(i).unwrap())))
            .collect();
        entries.into()
    };
    for writes in [2, 4, 8] {
        let base = flat_with_values(writes, &|_| wide(3_000));
        measure(&format!("{writes} maps of 3,000 keys"), &base, &trivial_update(&base));
    }
}

// ---------------------------------------------------------------------------------------------
// How much of a realistic write load stays incremental
// ---------------------------------------------------------------------------------------------

/// One simulated client: a document, what it has sent, and its write schedule.
struct SimClient {
    doc: LoroDoc,
    sent: loro::VersionVector,
    period_ms: u64,
    next_write_ms: u64,
}

fn client_write(client: &SimClient, rng: &mut super::window_fixtures::Rng) {
    let list = children(&client.doc);
    if rng.below(5) == 0 {
        let at = rng.index(list.len() + 1);
        let block = list.insert_container(at, LoroMap::new()).unwrap();
        block.insert("nodeName", "paragraph").unwrap();
        let inline = block.insert_container("children", loro::LoroList::new()).unwrap();
        inline
            .insert_container(0, loro::LoroText::new())
            .unwrap()
            .insert(0, "new block")
            .unwrap();
    } else {
        let index = rng.index(list.len());
        let Some(ValueOrContainer::Container(Container::Map(block))) = list.get(index) else {
            return;
        };
        let Some(ValueOrContainer::Container(Container::List(inline))) = block.get("children") else {
            return;
        };
        let Some(ValueOrContainer::Container(Container::Text(text))) = inline.get(0) else {
            return;
        };
        let at = text.len_unicode().min(rng.index(text.len_unicode() + 1));
        text.insert(at, "ab").unwrap();
    }
    client.doc.commit();
}

/// Two (or one) clients write to one page through the window as the worker runs it: the server's
/// base is the accepted head, a client's update goes to the server after `latency_ms` and the
/// server's accepted update reaches the other client after `latency_ms` more. Returns
/// (writes, incremental, fallbacks by reason).
fn simulate(
    clients: usize,
    period_ms: u64,
    latency_ms: u64,
    writes_each: usize,
    seed: u64,
) -> (usize, usize, std::collections::BTreeMap<String, usize>) {
    use crate::window_check::run_window;
    let mut rng = super::window_fixtures::Rng::new(seed);
    let base = flat_realistic(300);
    let mut server = base.clone();
    let mut pool: Vec<SimClient> = (0..clients)
        .map(|index| {
            let doc = LoroDoc::new();
            doc.import(&base).unwrap();
            doc.set_peer_id(100 + index as u64).unwrap();
            let sent = doc.oplog_vv();
            SimClient {
                doc,
                sent,
                period_ms,
                next_write_ms: rng.below(period_ms),
            }
        })
        .collect();
    // (arrival time, kind, client, bytes): 0 = an update arrives at the server, 1 = the server's
    // update reaches a client.
    let mut inbox: Vec<(u64, u8, usize, Vec<u8>)> = Vec::new();
    let mut writes = vec![0usize; clients];
    let mut incremental = 0;
    let mut total = 0;
    let mut fallbacks: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut now = 0;
    while writes.iter().any(|w| *w < writes_each) || !inbox.is_empty() {
        let next_write = pool
            .iter()
            .enumerate()
            .filter(|(i, _)| writes[*i] < writes_each)
            .map(|(i, c)| (c.next_write_ms, i))
            .min();
        let next_event = inbox.iter().enumerate().map(|(i, e)| (e.0, i)).min();
        match (next_write, next_event) {
            (Some((wt, ci)), ev) if ev.is_none_or(|(et, _)| wt <= et) => {
                now = wt;
                client_write(&pool[ci], &mut rng);
                let update = pool[ci].doc.export(ExportMode::updates(&pool[ci].sent)).unwrap();
                pool[ci].sent = pool[ci].doc.oplog_vv();
                pool[ci].next_write_ms = now + pool[ci].period_ms / 2 + rng.below(pool[ci].period_ms);
                writes[ci] += 1;
                inbox.push((now + latency_ms, 0, ci, update));
            }
            (_, Some((et, index))) => {
                now = et;
                let (_, kind, ci, bytes) = inbox.remove(index);
                if kind == 1 {
                    pool[ci].doc.import(&bytes).unwrap();
                    pool[ci].sent = pool[ci].doc.oplog_vv();
                    continue;
                }
                let mut engine = LoroCollabEngine::load(&server).unwrap();
                engine.prewarm_for_writes();
                let facts = crate::facts::BaseFacts::compute(&engine);
                crate::walk_counter::reset();
                let accepted = run_window(&mut engine, facts.as_ref(), &bytes).unwrap();
                total += 1;
                if accepted.report.is_some() {
                    incremental += 1;
                } else {
                    let reason = crate::walk_counter::fallback_reasons()
                        .last()
                        .cloned()
                        .unwrap_or_default();
                    *fallbacks.entry(reason).or_default() += 1;
                }
                server = engine.export_snapshot().unwrap();
                for other in 0..clients {
                    if other != ci {
                        inbox.push((now + latency_ms, 1, other, bytes.clone()));
                    }
                }
            }
            (Some(_) | None, None) => break,
        }
    }
    let _ = now;
    (total, incremental, fallbacks)
}

#[test]
#[ignore = "measurement, run in release"]
fn window_realistic_fraction() {
    println!("single client, typing (80%) and block inserts (20%), 300-block page");
    let (total, inc, fb) = simulate(1, 300, 40, 200, 1);
    println!(
        "  1 client: {inc} of {total} writes incremental ({:.0}%), fallbacks {fb:?}",
        100.0 * inc as f64 / total as f64
    );
    println!("two clients on one page: write period per client, one-way latency");
    for (period, latency) in [
        (200, 20),
        (200, 100),
        (500, 100),
        (1_000, 100),
        (200, 300),
        (2_000, 100),
        (5_000, 100),
    ] {
        let (mut total, mut inc) = (0, 0);
        let mut fb_all: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for seed in 1..=5 {
            let (t, i, fb) = simulate(2, period, latency, 100, seed);
            total += t;
            inc += i;
            for (k, v) in fb {
                *fb_all.entry(k).or_default() += v;
            }
        }
        println!(
            "  period {period:>5} ms, latency {latency:>3} ms: {inc} of {total} incremental ({:.0}%), fallbacks {fb_all:?}",
            100.0 * inc as f64 / total as f64
        );
    }
}

/// Prints, for a flat page of 50 blocks whose first block is the dense one, the items, the budget,
/// the full window (import, projection, limits) with its cost per item, to compare with the 0.58 us
/// the budget assumes, and the incremental window of a one-character edit in the dense block and in
/// another.
fn measure_dense_page(label: &str, base: &[u8]) {
    use super::window_round4_tests::first_text;
    let (items, budget) = super::window_round2_tests::items_and_budget(base);
    let edit = |block: usize, peer: u64| {
        raw_edit(base, peer, |raw| {
            first_text(raw, block).insert(1, "z").unwrap();
        })
    };
    let time = |update: &[u8], with_facts: bool| {
        let mut best = Duration::MAX;
        for _ in 0..5 {
            let mut engine = LoroCollabEngine::load(base).unwrap();
            engine.prewarm_for_writes();
            let facts = if with_facts {
                crate::facts::BaseFacts::compute(&engine)
            } else {
                None
            };
            let started = Instant::now();
            crate::window_check::run_window(&mut engine, facts.as_ref(), update).unwrap();
            best = best.min(started.elapsed());
        }
        best.as_secs_f64() * 1e3
    };
    let dense = edit(0, 47_900);
    let other = edit(1, 47_901);
    let full = time(&other, false);
    println!(
        "{label:<58} items {items:>7} / budget {budget:>7}  full window {full:7.2} ms ({:.3} us/item)  incremental: dense block {:7.2} ms, other block {:7.3} ms",
        full * 1e3 / items as f64,
        time(&dense, true),
        time(&other, true),
    );
}

/// The shapes that make a text expensive to convert, each grown until the page is at its work
/// budget (page of 50 blocks, the shape in the first block), so that the rows are comparable:
///
/// * `bold` on every eighth character under 15 full-range marks with fresh keys and the value
///   `false` (which the reader skips before it checks the key, so any number of keys is accepted);
/// * `bold` on alternate characters;
/// * segments that carry all seven marks the reader accepts (`strong`, `bold`, `em`, `italic`,
///   `strike`, `code`, `link`), every second segment;
/// * text typed in reverse order (one internal piece per character), without marks and under 15
///   full-range marks.
///
/// Times the full window and the incremental windows and prints the cost per item. Run in release.
#[test]
#[ignore = "calibration, run in release"]
fn window_dense_mark_calibration() {
    use super::window_round4_tests::first_text;
    use super::window_round6_tests::{configure_styles, mark_keys};
    let seven = ["strong", "bold", "em", "italic", "strike", "code", "link"].map(String::from);
    let mut all_keys = mark_keys(15);
    all_keys.extend(seven.iter().cloned());
    // Snapshots, not updates: the shapes below are far over the 64 KB update limit, which the
    // measurement does not need.
    let built = |peer: u64, edit: &dyn Fn(&LoroDoc)| {
        let raw = LoroDoc::new();
        raw.import(&super::window_round2_tests::flat_page_of(50)).unwrap();
        raw.set_peer_id(peer).unwrap();
        configure_styles(&raw, &all_keys);
        edit(&raw);
        raw.commit();
        raw.export(ExportMode::snapshot()).unwrap()
    };
    // The largest `units` for which the page is within its budget: doubling, then bisection.
    let at_budget = |label: &str, shape: &dyn Fn(&LoroDoc, usize)| {
        let page = |units: usize| built(47_000, &|raw| shape(raw, units));
        let within = |units: usize| {
            let (items, budget) = super::window_round2_tests::items_and_budget(&page(units));
            items <= budget
        };
        let (mut low, mut high) = (1, 2);
        while within(high) && high < 200_000 {
            low = high;
            high *= 2;
        }
        while high - low > 1 {
            let middle = usize::midpoint(low, high);
            if within(middle) {
                low = middle;
            } else {
                high = middle;
            }
        }
        measure_dense_page(&format!("{label} ({low} units)"), &page(low));
    };
    at_budget("bold on every 8th character, 15 false keys", &|raw, units| {
        let text = first_text(raw, 0);
        text.insert(1, &"y".repeat(8 * units - 1)).unwrap();
        for at in (0..8 * units).step_by(8) {
            text.mark(at..at + 1, "bold", true).unwrap();
        }
        for key in mark_keys(15) {
            text.mark(0..8 * units, &key, false).unwrap();
        }
    });
    at_budget("bold on alternate characters", &|raw, units| {
        let text = first_text(raw, 0);
        text.insert(1, &"y".repeat(2 * units - 1)).unwrap();
        for at in (0..2 * units).step_by(2) {
            text.mark(at..at + 1, "bold", true).unwrap();
        }
    });
    at_budget("seven marks on every second segment", &|raw, units| {
        let text = first_text(raw, 0);
        text.insert(1, &"y".repeat(4 * units - 1)).unwrap();
        for at in (0..4 * units).step_by(4) {
            for key in &seven {
                if key == "link" {
                    text.mark(at..at + 2, key, "https://example.test/a/page").unwrap();
                } else {
                    text.mark(at..at + 2, key, true).unwrap();
                }
            }
        }
    });
    for marks in [0, 15] {
        at_budget(&format!("typed in reverse, {marks} full-range marks"), &|raw, units| {
            let text = first_text(raw, 0);
            for _ in 0..units {
                text.insert(0, "y").unwrap();
            }
            for key in mark_keys(marks) {
                text.mark(0..text.len_unicode(), &key, false).unwrap();
            }
        });
    }
}

/// Times the setup of a page (the base facts and the host's projection) and its full window, and
/// prints the cost per item.
fn measure_fragmented(label: &str, base: &[u8]) {
    use super::window_round4_tests::first_text;
    let items = LoroCollabEngine::load(base)
        .unwrap()
        .projection_items(u64::MAX)
        .unwrap()
        .unwrap();
    let (mut projection, mut compute, mut full) = (Duration::MAX, Duration::MAX, Duration::MAX);
    let mut has_facts = false;
    let update = raw_edit(base, 49_900, |raw| {
        first_text(raw, 1).insert(1, "z").unwrap();
    });
    for _ in 0..3 {
        let engine = LoroCollabEngine::load(base).unwrap();
        let started = Instant::now();
        engine.semantic_snapshot().unwrap();
        projection = projection.min(started.elapsed());
        let engine = LoroCollabEngine::load(base).unwrap();
        engine.prewarm_for_writes();
        let started = Instant::now();
        has_facts = crate::facts::BaseFacts::compute(&engine).is_some();
        compute = compute.min(started.elapsed());
        let mut engine = LoroCollabEngine::load(base).unwrap();
        engine.prewarm_for_writes();
        let started = Instant::now();
        crate::window_check::run_window(&mut engine, None, &update).unwrap();
        full = full.min(started.elapsed());
    }
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    println!(
        "{label:<34} items {items:>8}  facts {has_facts:<5}  projection {:>8.2} ms ({:>6.3} us/item)  setup (count + projection + units) {:>8.2} ms  full window {:>8.2} ms  setup x3.6 {:>8.1} ms  window x3.6 {:>8.1} ms",
        ms(projection),
        ms(projection) * 1e3 / items.max(1) as f64,
        ms(compute),
        ms(full),
        ms(compute) * 3.6,
        ms(full) * 3.6,
    );
}

/// The fragmented shape: text typed in reverse order keeps one internal piece per character, and
/// converting it costs the pieces times its attributes. One block of 4,000 characters under K
/// full-range marks on a page of 50 blocks, then the review's page of 240 such blocks.
#[test]
#[ignore = "calibration, run in release"]
fn window_fragmented_calibration() {
    use super::window_round6_tests::fragmented_page;
    for marks in [0, 5, 10, 20, 40] {
        measure_fragmented(
            &format!("1 block, 4,000 chars, {marks} marks"),
            &fragmented_page(50, 1, 4_000, marks),
        );
    }
    // The review's page (240 blocks of 4,000 reverse-typed characters) is far over the budget; the
    // setup of a page over the budget stops at the ceiling.
    for marks in [0, 15] {
        measure_fragmented(
            &format!("240 blocks, 4,000 chars, {marks} marks"),
            &fragmented_page(240, 240, 4_000, marks),
        );
    }
    // The largest number of such blocks that fits the budget, on pages of 240 and 9,950 blocks.
    for (blocks, marks) in [(240, 0), (240, 15), (9_950, 0), (9_950, 15)] {
        let within = |fragmented: usize| {
            let (items, budget) =
                super::window_round2_tests::items_and_budget(&fragmented_page(blocks, fragmented, 4_000, marks));
            items <= budget
        };
        let (mut low, mut high) = (0, blocks.min(240));
        while high - low > 1 {
            let middle = usize::midpoint(low, high);
            if within(middle) {
                low = middle;
            } else {
                high = middle;
            }
        }
        measure_fragmented(
            &format!("{blocks} blocks, {low} x 4,000 chars, {marks} marks"),
            &fragmented_page(blocks, low, 4_000, marks),
        );
    }
}

/// Honest heavy formatting: items per block under the work measure, against the budget, and the cost
/// of the setup (count, projection and units) and of the full projection, for pages of 80-character
/// blocks. Run in release.
#[test]
#[ignore = "calibration, run in release"]
fn window_formatting_calibration() {
    use super::window_round6_tests::{Formatting, formatted_page};
    for (blocks, formatting) in [
        (1_200, Formatting::Clusters(1)),
        (1_200, Formatting::Clusters(2)),
        (2_000, Formatting::SixMarks),
        (2_000, Formatting::Clusters(2)),
        (9_950, Formatting::Clusters(1)),
        (9_950, Formatting::SixMarks),
        (9_950, Formatting::Clusters(2)),
    ] {
        let base = formatted_page(blocks, formatting);
        let (items, budget) = super::window_round2_tests::items_and_budget(&base);
        println!(
            "{blocks} blocks, {formatting:?}: {:.1} items per block, budget {:.1} per block, items {items}, budget {budget}",
            items as f64 / blocks as f64,
            budget as f64 / blocks as f64
        );
        measure_fragmented(&format!("{blocks} blocks {formatting:?}"), &base);
    }
}
