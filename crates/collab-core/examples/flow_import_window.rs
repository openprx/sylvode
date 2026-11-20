//! Local, in-process measurement of the isolated-apply worker's metered window for the Flow
//! write path: `import_client_update` -> `semantic_snapshot` -> `check_snapshot`,
//! on a base document loaded outside the window (exactly the worker's `decode_apply_and_check`
//! sequence, minus the `SIGPROF` timer and counting allocator).
//!
//! Run with:
//!   cargo run --release -p collab-core --example `flow_import_window`
//!
//! Numbers are CPU time (`CLOCK_PROCESS_CPUTIME_ID`), the quantity `ITIMER_PROF` enforces in
//! production, measured on whatever machine runs this binary. They are local numbers: the real
//! limit is only enforced by the Linux worker.
//!
//! Document shape mirrors `scripts/flow-bench/bench.py`: a Page whose prose lives in the editor
//! document, grown in batches of 50 blocks (`create_node` at index 0 plus `insert_text`). The
//! timed update is the last batch. Each size is measured for a flat model-1 Page (what the
//! benchmark creates), a structured model-4 Page, and a structured model-5 Page (one image atom
//! raises the floor; requesting model 5 alone leaves a plain structure at model 4).
//!
//! `FLOW_WINDOW_SAMPLES` (default 7) and `FLOW_WINDOW_MODELS` (comma-separated, default `1,4,5`)
//! narrow a run.
//!
//! `FLOW_WINDOW_PREWARM=0` skips `LoroCollabEngine::prewarm_for_writes()`, which the worker runs in
//! its setup before the window opens (default: on). The `raw` column is then measured on a document
//! prewarmed through the same public Loro reads, or cold, to match.
//!
//! The `incr` column is the window as the worker runs it since package C1: `window_check::run_window`
//! with the base facts computed in setup (`facts`, not part of the window), on a freshly loaded and
//! prewarmed base. `fb` counts the samples in which the incremental check did not decide and the
//! full computation did (structured pages, until the structured path exists, always do).
//! Model `0` is a legacy tree-only page of 10,000 nodes whose last update creates 1,000 nodes,
//! the shape of the structural-limit tests; `FLOW_WINDOW_MODELS=0` selects it.
//!
//! `FLOW_WINDOW_UPDATE=delete` replaces the timed update by one raw deletion of a top-level
//! block (`doc.children`), authored by a plain Loro peer on the base. The guard hands every such
//! deletion to its exact path: a narrow scan of the block list and, on a page without the model 5
//! marker or a remaining atom, a read of the pre-import state. Use it with `FLOW_WINDOW_MODELS=1`: on
//! a structured page a raw list deletion leaves a dangling placement and the projection refuses the
//! page, so there is nothing to time. This is the cost of the worst accepted shape, not of an
//! ordinary edit.
//!
//! Besides the window, the binary reports standalone costs of the parts inside it:
//! - `raw`: `LoroDoc::import` of the same update on a freshly loaded base, with nothing around it;
//! - `guard`: `import - raw`, the cost the update-scoped downgrade guard (plus the id-cache
//!   rebuild) adds inside `import_client_update`;
//! - `floor_pre`: a full `minimum_editor_model` walk on a freshly loaded base, the work the old
//!   importer did before every import (kept as the reference cost);
//! - `floor_post`: `minimum_editor_model` on the imported state after the window, which the fused
//!   projection has already memoized, so it costs no walk;
//! - a host-side rebuild that replays the last `REBUILD_TAIL` batches onto an older snapshot with
//!   `import_update`.

#![allow(unsafe_code)]

use std::io::Write as _;
use std::time::Duration;

use collab_core::facts::BaseFacts;
use collab_core::limits::{DocumentLimits, check_snapshot};
use collab_core::window_check::run_window;
use collab_core::{CollabEngine, LoroCollabEngine, NodeId, NodeKind, Operation};

const BLOCKS_PER_BATCH: usize = 50;
const SIZES: [usize; 3] = [1_000, 5_000, 9_950];
const REBUILD_TAIL: usize = 20;

fn cpu_time_now() -> Result<Duration, String> {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `&raw mut ts` is a valid, properly aligned out-pointer for a `timespec`;
    // `CLOCK_PROCESS_CPUTIME_ID` is supported on Linux and macOS.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &raw mut ts) };
    if rc != 0 {
        return Err("clock_gettime(CLOCK_PROCESS_CPUTIME_ID) failed".to_string());
    }
    let secs = u64::try_from(ts.tv_sec).map_err(|_| "negative tv_sec".to_string())?;
    let nanos = u32::try_from(ts.tv_nsec).map_err(|_| "invalid tv_nsec".to_string())?;
    Ok(Duration::new(secs, nanos))
}

fn timed<T>(f: impl FnOnce() -> Result<T, String>) -> Result<(T, f64), String> {
    let start = cpu_time_now()?;
    let value = f()?;
    let end = cpu_time_now()?;
    Ok((value, end.saturating_sub(start).as_secs_f64() * 1000.0))
}

fn median(samples: &mut [f64]) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples.get(samples.len() / 2).copied().unwrap_or(f64::NAN)
}

fn batch_ops(start: usize) -> Vec<Operation> {
    let mut ops = Vec::with_capacity(BLOCKS_PER_BATCH * 2);
    for index in start..start + BLOCKS_PER_BATCH {
        let id = NodeId::from(format!("b{index:05}"));
        ops.push(Operation::CreateNode {
            id: id.clone(),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        });
        ops.push(Operation::InsertText {
            id,
            index: 0,
            text: format!("benchblock {index} alpha"),
        });
    }
    ops
}

/// An update from a plain Loro peer that deletes one top-level block of the base document.
fn raw_block_deletion(base: &[u8]) -> Result<Vec<u8>, String> {
    let doc = loro::LoroDoc::new();
    collab_core::page_inline::configure(&doc);
    doc.import(base).map_err(|e| e.to_string())?;
    doc.set_peer_id(9_999).map_err(|e| e.to_string())?;
    let from = doc.oplog_vv();
    let Some(loro::ValueOrContainer::Container(loro::Container::List(list))) = doc.get_map("doc").get("children")
    else {
        return Err("the base has no editor block list".to_string());
    };
    list.delete(3, 1).map_err(|e| e.to_string())?;
    doc.commit();
    doc.export(loro::ExportMode::updates(&from)).map_err(|e| e.to_string())
}

fn prewarm_enabled() -> bool {
    std::env::var("FLOW_WINDOW_PREWARM").map_or(true, |value| value != "0")
}

/// The same reads as `LoroCollabEngine::prewarm_for_writes`, on a bare Loro document.
fn prewarm_raw(doc: &loro::LoroDoc) {
    use loro::{Container, Index, ValueOrContainer};
    if let Some(ValueOrContainer::Container(Container::Map(root))) = doc.get_by_path(&[Index::Key("doc".into())])
        && let Some(ValueOrContainer::Container(Container::List(list))) = root.get("children")
    {
        let _ = list.get_id_at(0);
    }
    for name in ["flow_editor_aliases", "order"] {
        if let Some(ValueOrContainer::Container(Container::Map(map))) = doc.get_by_path(&[Index::Key(name.into())]) {
            let _ = map.get_last_editor("");
        }
    }
    if let Some(ValueOrContainer::Container(Container::Tree(tree))) =
        doc.get_by_path(&[Index::Key("flow_editor_structure".into())])
    {
        let _ = tree.roots();
    }
}

struct Fixture {
    /// Snapshot before the last batch: the worker's base document.
    base: Vec<u8>,
    /// The last batch: the worker's untrusted update.
    update: Vec<u8>,
    /// Snapshot before the last `REBUILD_TAIL` batches, plus those batches as separate updates.
    rebuild_base: Vec<u8>,
    rebuild_tail: Vec<Vec<u8>>,
}

/// A legacy tree-only page of `nodes` nodes whose last update creates 1,000 of them.
fn build_legacy(nodes: usize) -> Result<Fixture, String> {
    let err = |e: collab_core::CollabError| e.to_string();
    let mut engine = LoroCollabEngine::new_empty(1);
    let create = |engine: &mut LoroCollabEngine, index: usize| {
        engine
            .apply_operation(&Operation::CreateNode {
                id: NodeId::from(format!("n{index:05}")),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            })
            .map_err(|e| e.to_string())
    };
    let chunk = 1_000.min(nodes);
    for index in 0..nodes - chunk {
        create(&mut engine, index)?;
    }
    let base = engine.export_snapshot().map_err(err)?;
    let before = engine.frontier();
    for index in nodes - chunk..nodes {
        create(&mut engine, index)?;
    }
    let update = engine.export_from(&before).map_err(err)?;
    Ok(Fixture {
        base: base.clone(),
        update,
        rebuild_base: base,
        rebuild_tail: Vec::new(),
    })
}

fn build(blocks: usize, model: u32) -> Result<Fixture, String> {
    if model == 0 {
        return build_legacy(blocks);
    }
    let err = |e: collab_core::CollabError| e.to_string();
    let mut engine = LoroCollabEngine::new_empty(1);
    engine.initialize_page_editor().map_err(err)?;
    if model >= 4 {
        engine.initialize_page_structure_model(model).map_err(err)?;
    }
    if model >= 5 {
        engine
            .insert_page_atom(
                "bench-image",
                None,
                0,
                "image",
                &serde_json::json!({"asset_id": "00000000-0000-4000-8000-000000000001", "alt": "", "caption": "", "indent": 0}),
            )
            .map_err(err)?;
    }
    if engine.minimum_editor_model().map_err(err)? != model {
        return Err(format!("fixture did not reach editor model {model}"));
    }
    let batches = blocks / BLOCKS_PER_BATCH;
    let mut rebuild_base = Vec::new();
    let mut rebuild_tail = Vec::new();
    let mut base = Vec::new();
    let mut update = Vec::new();
    for batch in 0..batches {
        if batch + REBUILD_TAIL == batches {
            rebuild_base = engine.export_snapshot().map_err(err)?;
        }
        if batch + 1 == batches {
            base = engine.export_snapshot().map_err(err)?;
        }
        let before = engine.frontier();
        for op in batch_ops(batch * BLOCKS_PER_BATCH) {
            engine.apply_content_operation(&op).map_err(err)?;
        }
        if batch + REBUILD_TAIL >= batches {
            let delta = engine.export_from(&before).map_err(err)?;
            if batch + 1 == batches {
                update.clone_from(&delta);
            }
            rebuild_tail.push(delta);
        }
    }
    if std::env::var("FLOW_WINDOW_UPDATE").is_ok_and(|value| value == "delete") {
        update = raw_block_deletion(&base)?;
    }
    Ok(Fixture {
        base,
        update,
        rebuild_base,
        rebuild_tail,
    })
}

#[derive(Default)]
struct Samples {
    import: Vec<f64>,
    raw: Vec<f64>,
    guard: Vec<f64>,
    semantic: Vec<f64>,
    check: Vec<f64>,
    window: Vec<f64>,
    incr: Vec<f64>,
    facts: Vec<f64>,
    fallbacks: usize,
    floor_pre: Vec<f64>,
    floor_post: Vec<f64>,
    rebuild: Vec<f64>,
}

fn sample(fixture: &Fixture, samples: &mut Samples) -> Result<(), String> {
    let err = |e: collab_core::CollabError| e.to_string();
    // The window, exactly as the worker runs it.
    let mut engine = LoroCollabEngine::load(&fixture.base).map_err(err)?;
    if prewarm_enabled() {
        engine.prewarm_for_writes();
    }
    let ((), import) = timed(|| engine.import_client_update(&fixture.update).map(|_| ()).map_err(err))?;
    let (semantic, semantic_ms) = timed(|| engine.semantic_snapshot().map_err(err))?;
    let ((), check) = timed(|| check_snapshot(&semantic, &DocumentLimits::default()).map_err(|v| format!("{v:?}")))?;
    // The incremental window, as the worker runs it since package C1.
    let mut incremental = LoroCollabEngine::load(&fixture.base).map_err(err)?;
    if prewarm_enabled() {
        incremental.prewarm_for_writes();
    }
    let (facts, facts_ms) = timed(|| Ok(BaseFacts::compute(&incremental)))?;
    let (accepted, incr_ms) = timed(|| run_window(&mut incremental, facts.as_ref(), &fixture.update).map_err(err))?;
    samples.incr.push(incr_ms);
    samples.facts.push(facts_ms);
    samples.fallbacks += usize::from(accepted.fallback.is_some());
    // The same update applied by Loro alone, on its own freshly loaded copy of the base.
    let raw_doc = loro::LoroDoc::new();
    collab_core::page_inline::configure(&raw_doc);
    raw_doc.import(&fixture.base).map_err(|e| e.to_string())?;
    if prewarm_enabled() {
        prewarm_raw(&raw_doc);
    }
    let ((), raw) = timed(|| raw_doc.import(&fixture.update).map(|_| ()).map_err(|e| e.to_string()))?;
    samples.raw.push(raw);
    samples.guard.push((import - raw).max(0.0));
    samples.import.push(import);
    samples.semantic.push(semantic_ms);
    samples.check.push(check);
    samples.window.push(import + semantic_ms + check);

    // Standalone floor walks: cold on a freshly loaded base, and on the imported state.
    let probe = LoroCollabEngine::load(&fixture.base).map_err(err)?;
    let (_, floor_pre) = timed(|| probe.minimum_editor_model().map_err(err))?;
    samples.floor_pre.push(floor_pre);
    let (_, floor_post) = timed(|| engine.minimum_editor_model().map_err(err))?;
    samples.floor_post.push(floor_post);

    // Host-side rebuild: snapshot plus a tail of accepted updates.
    let mut rebuilt = LoroCollabEngine::load(&fixture.rebuild_base).map_err(err)?;
    let ((), rebuild_ms) = timed(|| {
        for update in &fixture.rebuild_tail {
            rebuilt.import_update(update).map_err(err)?;
        }
        Ok(())
    })?;
    samples.rebuild.push(rebuild_ms);
    Ok(())
}

fn main() {
    let samples_per_size = std::env::var("FLOW_WINDOW_SAMPLES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(7);
    let models: Vec<u32> = std::env::var("FLOW_WINDOW_MODELS")
        .unwrap_or_else(|_| "1,4,5".to_string())
        .split(',')
        .filter_map(|value| value.trim().parse().ok())
        .collect();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = writeln!(
        out,
        "local CPU ms, median of {samples_per_size}; window = import + semantic_snapshot + check_snapshot"
    );
    let _ = writeln!(
        out,
        "model blocks | window incr(fb) facts | import raw guard semantic check | floor_pre(cold) floor_post | rebuild({REBUILD_TAIL} tail)"
    );
    for model in models.iter().copied() {
        let sizes: &[usize] = if model == 0 { &[5_000, 10_000] } else { &SIZES };
        for blocks in sizes.iter().copied() {
            let fixture = match build(blocks, model) {
                Ok(fixture) => fixture,
                Err(error) => {
                    let _ = writeln!(out, "model {model} blocks {blocks}: build failed: {error}");
                    continue;
                }
            };
            let mut samples = Samples::default();
            for _ in 0..samples_per_size {
                if let Err(error) = sample(&fixture, &mut samples) {
                    let _ = writeln!(out, "model {model} blocks {blocks}: sample failed: {error}");
                    break;
                }
            }
            let _ = writeln!(
                out,
                "{model:>5} {blocks:>6} | {:>6.2} {:>6.2}({}) {:>6.1} | {:>6.2} {:>5.2} {:>5.2} {:>8.2} {:>5.2} | {:>15.2} {:>10.2} | {:>8.2}",
                median(&mut samples.window),
                median(&mut samples.incr),
                samples.fallbacks,
                median(&mut samples.facts),
                median(&mut samples.import),
                median(&mut samples.raw),
                median(&mut samples.guard),
                median(&mut samples.semantic),
                median(&mut samples.check),
                median(&mut samples.floor_pre),
                median(&mut samples.floor_post),
                median(&mut samples.rebuild),
            );
        }
    }
}
