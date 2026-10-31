//! Times the production Loro adapter: apply a block batch, export a snapshot, load it,
//! and import one follow-up update onto that loaded document.
//!
//! Writes a JSON object to the path given as the only argument. The GitHub Actions
//! workflow runs this binary; it is not a microbenchmark harness (`cargo bench`).

#![allow(
    clippy::pedantic,
    reason = "measurement binary; the workspace deny lints still apply"
)]
#![allow(clippy::nursery, reason = "measurement binary; the workspace deny lints still apply")]

use std::hint::black_box;
use std::time::Instant;

use collab_core::{CollabEngine, CollabError, LoroCollabEngine, NodeId, NodeKind, Operation};

const BLOCK_COUNTS: [usize; 2] = [1_000, 5_000];
const APPLY_SAMPLES: usize = 3;
const REPLAY_SAMPLES: usize = 5;

fn main() -> anyhow::Result<()> {
    let output = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: flow-loro-bench OUTPUT.json"))?;
    let mut sizes = Vec::with_capacity(BLOCK_COUNTS.len());
    for blocks in BLOCK_COUNTS {
        sizes.push(measure(blocks)?);
    }
    let report = serde_json::json!({
        "ok": true,
        "engine": "collab-core::LoroCollabEngine",
        "loro": "1.13.9",
        "apply_samples": APPLY_SAMPLES,
        "replay_samples": REPLAY_SAMPLES,
        "sizes": sizes,
    });
    std::fs::write(&output, serde_json::to_vec_pretty(&report)?)?;
    Ok(())
}

fn measure(blocks: usize) -> anyhow::Result<serde_json::Value> {
    let ops = build_ops(blocks)?;
    let (head, tail) = split_tail(&ops)?;

    let mut apply_ms = Vec::with_capacity(APPLY_SAMPLES);
    for sample in 0..APPLY_SAMPLES {
        let seed = u64::try_from(sample).map_err(|_| anyhow::anyhow!("sample index"))? + 1;
        let mut engine = LoroCollabEngine::new_empty(seed);
        let started = Instant::now();
        apply_all(&mut engine, &ops)?;
        apply_ms.push(elapsed_ms(started));
        std::mem::drop(black_box(engine.frontier()));
    }

    let mut base = LoroCollabEngine::new_empty(11);
    apply_all(&mut base, head)?;
    let frontier = base.frontier();
    let base_snapshot = base.export_snapshot().map_err(collab_err)?;
    apply_all(&mut base, tail)?;
    let update = base.export_from(&frontier).map_err(collab_err)?;
    let full_snapshot = base.export_snapshot().map_err(collab_err)?;

    let mut snapshot_ms = Vec::with_capacity(REPLAY_SAMPLES);
    let mut load_ms = Vec::with_capacity(REPLAY_SAMPLES);
    let mut import_ms = Vec::with_capacity(REPLAY_SAMPLES);
    for _ in 0..REPLAY_SAMPLES {
        let started = Instant::now();
        let exported = base.export_snapshot().map_err(collab_err)?;
        snapshot_ms.push(elapsed_ms(started));
        std::mem::drop(black_box(exported.len()));

        let started = Instant::now();
        let loaded = LoroCollabEngine::load(&full_snapshot).map_err(collab_err)?;
        load_ms.push(elapsed_ms(started));
        std::mem::drop(black_box(loaded.frontier()));

        let mut target = LoroCollabEngine::load(&base_snapshot).map_err(collab_err)?;
        let started = Instant::now();
        target.import_update(&update).map_err(collab_err)?;
        import_ms.push(elapsed_ms(started));
        std::mem::drop(black_box(target.frontier()));
    }

    let apply = stats(&apply_ms)?;
    let snapshot = stats(&snapshot_ms)?;
    let load = stats(&load_ms)?;
    let import_update = stats(&import_ms)?;
    Ok(serde_json::json!({
        "blocks": blocks,
        "operations": ops.len(),
        "apply": apply,
        "snapshot": snapshot,
        "load": load,
        "import_update": import_update,
        "snapshot_bytes": full_snapshot.len(),
        "base_snapshot_bytes": base_snapshot.len(),
        "update_bytes": update.len(),
    }))
}

fn build_ops(blocks: usize) -> anyhow::Result<Vec<Operation>> {
    let mut ops = Vec::new();
    for index in 0..blocks {
        let id = NodeId::from(format!("b{index}"));
        let position = u32::try_from(index).map_err(|_| anyhow::anyhow!("block index does not fit u32"))?;
        ops.push(Operation::CreateNode {
            id: NodeId::clone(&id),
            parent: None,
            index: position,
            kind: NodeKind::Block,
        });
        ops.push(Operation::InsertText {
            id,
            index: 0,
            text: format!("benchblock {index}"),
        });
    }
    Ok(ops)
}

fn split_tail(ops: &[Operation]) -> anyhow::Result<(&[Operation], &[Operation])> {
    if ops.len() < 2 {
        anyhow::bail!("need at least two operations");
    }
    let split = ops.len() - 2;
    let head = ops
        .get(..split)
        .ok_or_else(|| anyhow::anyhow!("missing operation head"))?;
    let tail = ops
        .get(split..)
        .ok_or_else(|| anyhow::anyhow!("missing operation tail"))?;
    Ok((head, tail))
}

fn apply_all(engine: &mut LoroCollabEngine, ops: &[Operation]) -> anyhow::Result<()> {
    for operation in ops {
        engine.apply_operation(operation).map_err(collab_err)?;
    }
    Ok(())
}

fn stats(samples: &[f64]) -> anyhow::Result<serde_json::Value> {
    let mut ordered = samples.to_vec();
    if ordered.is_empty() {
        anyhow::bail!("no samples");
    }
    ordered.sort_by(f64::total_cmp);
    let min = ordered.first().copied().ok_or_else(|| anyhow::anyhow!("min"))?;
    let max = ordered.last().copied().ok_or_else(|| anyhow::anyhow!("max"))?;
    let last = ordered.len().saturating_sub(1);
    let p50 = ordered
        .get(last.saturating_mul(50) / 100)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("p50"))?;
    let p95 = ordered
        .get(last.saturating_mul(95) / 100)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("p95"))?;
    Ok(serde_json::json!({
        "n": ordered.len(),
        "min_ms": min,
        "p50_ms": p50,
        "p95_ms": p95,
        "max_ms": max,
        "samples_ms": ordered,
    }))
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

fn collab_err(err: CollabError) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}
