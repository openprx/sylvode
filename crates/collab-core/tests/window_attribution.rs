//! Attribution of the isolated decode/apply window per page shape and size. Measurement only: it
//! is `#[ignore]`d and run on purpose by `scripts/flow-bench/window-attribution.sh` (release build,
//! Linux container). It prints tab-separated `WA_LIB` and `WA_WORKER` lines that the script turns
//! into a table; it asserts nothing about timing.
//!
//! Each cell is a page of `blocks` blocks reached by 50-block batches (two operations per block,
//! as the benchmark sends them), measured on the last batch:
//!
//! - `WA_LIB`: the window's three parts in this process, without any ceiling, as CPU time
//!   (`CLOCK_PROCESS_CPUTIME_ID`): `import_client_update`, `semantic_snapshot`, `check_snapshot`.
//!   Works at any size, including sizes the worker's 50 ms ceiling would kill.
//! - `WA_WORKER`: the same update through the real worker process (`isolated_apply`), reporting
//!   the worker's own `window_cpu_us` and the child's total CPU (`RUSAGE_CHILDREN`: process start,
//!   request read, base load, window, export). A ceiling kill is reported as the status.
//!
//! Environment: `WINDOW_ATTR_SIZES` (comma list, default `1000,2700,5000,10000`; each a multiple
//! of 50), `WINDOW_ATTR_SHAPES` (`flat`, `structured`; default both), `WINDOW_ATTR_RUNS` (default
//! 7).

#![allow(
    unsafe_code,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]

use collab_core::isolation::isolated_apply;
use collab_core::limits::{DocumentLimits, check_snapshot};
use collab_core::{CollabEngine, LoroCollabEngine, NodeId, NodeKind, Operation};

const BLOCKS_PER_BATCH: usize = 50;

fn process_cpu_us() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: valid out-pointer for a `timespec`; the clock id is supported on Linux and macOS.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &raw mut ts) };
    assert_eq!(rc, 0, "clock_gettime(CLOCK_PROCESS_CPUTIME_ID) failed");
    ts.tv_sec as u64 * 1_000_000 + ts.tv_nsec as u64 / 1_000
}

fn children_cpu_us() -> u64 {
    // SAFETY: zeroed `rusage` is a valid out-parameter for `getrusage`.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `&raw mut usage` is a valid out-pointer.
    let rc = unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, &raw mut usage) };
    assert_eq!(rc, 0, "getrusage(RUSAGE_CHILDREN) failed");
    let tv = |t: libc::timeval| t.tv_sec as u64 * 1_000_000 + t.tv_usec as u64;
    tv(usage.ru_utime) + tv(usage.ru_stime)
}

fn timed<T>(work: impl FnOnce() -> T) -> (T, u64) {
    let before = process_cpu_us();
    let value = work();
    (value, process_cpu_us().saturating_sub(before))
}

fn median(values: &mut [u64]) -> u64 {
    values.sort_unstable();
    values.get(values.len() / 2).copied().unwrap_or(0)
}

struct Fixture {
    base: Vec<u8>,
    update: Vec<u8>,
}

fn batch(start: usize) -> Vec<Operation> {
    let mut operations = Vec::with_capacity(BLOCKS_PER_BATCH * 2);
    for index in start..start + BLOCKS_PER_BATCH {
        let id = NodeId::from(format!("b{index:05}"));
        operations.push(Operation::CreateNode {
            id: id.clone(),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        });
        operations.push(Operation::InsertText {
            id,
            index: 0,
            text: format!("benchblock {index} alpha"),
        });
    }
    operations
}

/// `model` 1 is the flat page the benchmark creates; 4 is a structured (placement tree) page.
fn build(blocks: usize, model: u32) -> Fixture {
    let mut engine = LoroCollabEngine::new_empty(1);
    engine.initialize_page_editor().expect("editor document initialises");
    if model >= 4 {
        engine
            .initialize_page_structure_model(model)
            .expect("structure model initialises");
    }
    let batches = blocks / BLOCKS_PER_BATCH;
    let mut base = Vec::new();
    let mut update = Vec::new();
    for index in 0..batches {
        if index + 1 == batches {
            base = engine.export_snapshot().expect("base exports");
        }
        let before = engine.frontier();
        for operation in batch(index * BLOCKS_PER_BATCH) {
            engine.apply_content_operation(&operation).expect("operation applies");
        }
        if index + 1 == batches {
            update = engine.export_from(&before).expect("update exports");
        }
    }
    Fixture { base, update }
}

fn env_list(name: &str, default: &str) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

#[test]
#[ignore = "measurement, run by scripts/flow-bench/window-attribution.sh"]
fn window_attribution() {
    let sizes: Vec<usize> = env_list("WINDOW_ATTR_SIZES", "1000,2700,5000,10000")
        .iter()
        .map(|item| item.parse().expect("size is a number"))
        .collect();
    let shapes = env_list("WINDOW_ATTR_SHAPES", "flat,structured");
    let runs: usize = std::env::var("WINDOW_ATTR_RUNS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(7);
    let limits = DocumentLimits::default();

    println!(
        "WA_HEADER\tbuild={}",
        if cfg!(debug_assertions) { "debug" } else { "release" }
    );
    for shape in &shapes {
        let model = match shape.as_str() {
            "flat" => 1,
            "structured" => 4,
            other => panic!("unknown shape {other}"),
        };
        for &blocks in &sizes {
            assert_eq!(
                blocks % BLOCKS_PER_BATCH,
                0,
                "size must be a multiple of {BLOCKS_PER_BATCH}"
            );
            let fixture = build(blocks, model);

            let (mut imports, mut snapshots, mut checks, mut windows) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for _ in 0..runs {
                let mut engine = LoroCollabEngine::load(&fixture.base).expect("base loads");
                let (imported, import_us) = timed(|| engine.import_client_update(&fixture.update));
                imported.expect("update imports");
                let (snapshot, snapshot_us) = timed(|| engine.semantic_snapshot());
                let snapshot = snapshot.expect("snapshot projects");
                let (checked, check_us) = timed(|| check_snapshot(&snapshot, &limits));
                let check_status = if checked.is_ok() { "" } else { "check_failed" };
                imports.push(import_us);
                snapshots.push(snapshot_us);
                checks.push(check_us);
                windows.push(import_us + snapshot_us + check_us);
                if !check_status.is_empty() {
                    println!("WA_NOTE\t{shape}\t{blocks}\tcheck_snapshot rejected the page");
                }
            }
            let window_min = windows.iter().copied().min().unwrap_or(0);
            let window_max = windows.iter().copied().max().unwrap_or(0);
            println!(
                "WA_LIB\t{shape}\t{blocks}\t{runs}\t{}\t{}\t{}\t{}\t{window_min}\t{window_max}",
                median(&mut imports),
                median(&mut snapshots),
                median(&mut checks),
                median(&mut windows),
            );

            let (mut worker_windows, mut child_totals) = (Vec::new(), Vec::new());
            let mut status = "ok".to_string();
            for _ in 0..runs {
                let children_before = children_cpu_us();
                match isolated_apply(&fixture.base, &fixture.update) {
                    Ok(success) => {
                        child_totals.push(children_cpu_us().saturating_sub(children_before));
                        match success.window_cpu_us {
                            Some(micros) => worker_windows.push(micros),
                            None => status = "no_window_cpu_us".to_string(),
                        }
                    }
                    Err(error) => {
                        status = format!("{error:?}");
                        break;
                    }
                }
            }
            let worker_min = worker_windows.iter().copied().min().unwrap_or(0);
            let worker_max = worker_windows.iter().copied().max().unwrap_or(0);
            println!(
                "WA_WORKER\t{shape}\t{blocks}\t{}\t{runs}\t{}\t{worker_min}\t{worker_max}\t{}\t{}",
                worker_windows.len(),
                median(&mut worker_windows),
                median(&mut child_totals),
                status.replace(['\t', '\n'], " "),
            );
        }
    }
}
