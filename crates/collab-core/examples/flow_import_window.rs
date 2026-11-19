//! Local, in-process measurement of the isolated-apply worker's metered window for the Flow
//! write path: `import_update_causally_complete` -> `semantic_snapshot` -> `check_snapshot`,
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
//! Besides the window, the binary reports standalone costs of the parts inside it:
//! `minimum_editor_model` on the freshly loaded base (the pre-import floor), the same call on the
//! imported state (the post-import floor check model-5 documents perform), and a host-side
//! rebuild that replays the last `REBUILD_TAIL` batches onto an older snapshot with
//! `import_update`.

#![allow(unsafe_code)]

use std::io::Write as _;
use std::time::Duration;

use collab_core::limits::{DocumentLimits, check_snapshot};
use collab_core::{CollabEngine, LoroCollabEngine, NodeId, NodeKind, Operation};

const BLOCKS_PER_BATCH: usize = 50;
const SIZES: [usize; 2] = [1_400, 5_000];
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

struct Fixture {
    /// Snapshot before the last batch: the worker's base document.
    base: Vec<u8>,
    /// The last batch: the worker's untrusted update.
    update: Vec<u8>,
    /// Snapshot before the last `REBUILD_TAIL` batches, plus those batches as separate updates.
    rebuild_base: Vec<u8>,
    rebuild_tail: Vec<Vec<u8>>,
}

fn build(blocks: usize, model: u32) -> Result<Fixture, String> {
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
    semantic: Vec<f64>,
    check: Vec<f64>,
    window: Vec<f64>,
    floor_pre: Vec<f64>,
    floor_post: Vec<f64>,
    rebuild: Vec<f64>,
}

fn sample(fixture: &Fixture, samples: &mut Samples) -> Result<(), String> {
    let err = |e: collab_core::CollabError| e.to_string();
    // The window, exactly as the worker runs it.
    let mut engine = LoroCollabEngine::load(&fixture.base).map_err(err)?;
    let ((), import) = timed(|| {
        engine
            .import_update_causally_complete(&fixture.update)
            .map(|_| ())
            .map_err(err)
    })?;
    let (semantic, semantic_ms) = timed(|| engine.semantic_snapshot().map_err(err))?;
    let ((), check) = timed(|| check_snapshot(&semantic, &DocumentLimits::default()).map_err(|v| format!("{v:?}")))?;
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
        "model blocks | window import semantic check | floor_pre(cold) floor_post | rebuild({REBUILD_TAIL} tail)"
    );
    for model in models.iter().copied() {
        for blocks in SIZES {
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
                "{model:>5} {blocks:>6} | {:>6.2} {:>6.2} {:>8.2} {:>5.2} | {:>15.2} {:>10.2} | {:>8.2}",
                median(&mut samples.window),
                median(&mut samples.import),
                median(&mut samples.semantic),
                median(&mut samples.check),
                median(&mut samples.floor_pre),
                median(&mut samples.floor_post),
                median(&mut samples.rebuild),
            );
        }
    }
}
